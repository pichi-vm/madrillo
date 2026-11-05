#![forbid(unsafe_code)]
#[cfg(target_os = "linux")]
mod cgroup;
mod connections;
mod framing;
mod input;
#[cfg(unix)]
mod launch;
mod logging;
mod process;
mod resources;
mod state;
mod terminal;
mod transport;
use clap::Parser;
use connections::Connections;
use input::Input;
use logging::{
    Capture,
    cri::Stream,
    file::{FileLog, Limits},
};
use madrillo::{
    CleanupResult, ClientMessage, Completion, CompletionCause, ErrorCode, LeaderResult,
    OomEvidence, PROTOCOL_VERSION, ServerMessage,
};
use process::Workload;
use state::State;
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
    process::{ExitCode, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Parser)]
#[command(version, about = "Native process monitor (under construction)")]
struct Cli {
    /// Print the shared protocol version.
    #[arg(long)]
    api_version: bool,
    /// New private directory for this workload's retained state.
    #[arg(long, required_unless_present = "api_version")]
    state_dir: Option<PathBuf>,
    /// Working directory for the child; environment is inherited unchanged.
    #[arg(long)]
    cwd: Option<PathBuf>,
    /// Private Unix socket or Windows named-pipe endpoint for live attachment.
    #[arg(long)]
    listen: Option<PathBuf>,
    /// Ownership of the already-connected initial control channel.
    #[arg(long)]
    control: Option<usize>,
    /// Wait for Start on the initial channel before executing the workload.
    #[arg(long, requires = "control")]
    start_gated: bool,
    /// Give the initial attachment input ownership.
    #[arg(long)]
    stdin: bool,
    /// Allocate a native host terminal. Pipe capture remains the default.
    #[arg(long)]
    terminal: bool,
    /// Initial terminal size, ROWSxCOLS.
    #[arg(long, default_value = "24x80", value_parser = parse_size)]
    size: (u16, u16),
    /// File path, k8s-file:PATH, journald, none, or passthrough. Repeat for file+journal.
    #[arg(long)]
    log_path: Vec<OsString>,
    /// Workload ID for conmon-compatible journal metadata.
    #[arg(long)]
    log_id: Option<String>,
    #[arg(long)]
    log_name: Option<String>,
    #[arg(long)]
    log_tag: Option<String>,
    #[arg(long)]
    log_label: Vec<String>,
    #[arg(long)]
    no_container_partial_message: bool,
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    log_size_max: Option<u64>,
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    log_global_size_max: Option<u64>,
    #[arg(long)]
    log_rotate: bool,
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u16).range(1..=1000))]
    log_max_files: u16,
    #[arg(long)]
    no_sync_log: bool,
    /// Existing, empty, caller-owned Linux cgroup v2 directory for this workload.
    #[arg(long)]
    cgroup: Option<PathBuf>,
    /// Force the workload scope after this many seconds.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    timeout: Option<u64>,
    /// Program and native arguments, following --.
    #[arg(last = true, required_unless_present = "api_version")]
    command: Vec<OsString>,
}

fn main() -> ExitCode {
    #[cfg(unix)]
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("__madrillo_exec")) {
        return match launch::exec() {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => ExitCode::FAILURE,
        };
    }
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr().lock(), "madrillo: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse_size(value: &str) -> Result<(u16, u16), String> {
    let (rows, cols) = value.split_once('x').ok_or("size must be ROWSxCOLS")?;
    let rows = rows.parse::<u16>().map_err(|e| e.to_string())?;
    let cols = cols.parse::<u16>().map_err(|e| e.to_string())?;
    if rows == 0 || cols == 0 {
        return Err("terminal dimensions must be nonzero".into());
    }
    Ok((rows, cols))
}

fn log_file_path(value: &std::ffi::OsStr) -> io::Result<PathBuf> {
    if value.is_empty() {
        return Err(io::Error::other("log path must not be empty"));
    }
    if !value.as_encoded_bytes().starts_with(b"k8s-file:") {
        return Ok(PathBuf::from(value));
    }
    #[cfg(unix)]
    let path = {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        OsString::from_vec(value.as_bytes()[9..].to_vec())
    };
    #[cfg(windows)]
    let path = {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        OsString::from_wide(&value.encode_wide().skip(9).collect::<Vec<_>>())
    };
    if path.is_empty() {
        return Err(io::Error::other("k8s-file needs a path"));
    }
    Ok(PathBuf::from(path))
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    if cli.api_version {
        writeln!(io::stdout().lock(), "{PROTOCOL_VERSION}")?;
        return Ok(());
    }
    // Adopt before opening state, endpoints, logs or any worker resources.
    let initial = cli.control.map(resources::control).transpose()?;
    let directory = cli.state_dir.as_ref().ok_or("missing state directory")?;
    let state = State::create(directory)?;
    #[cfg(unix)]
    let endpoint = cli
        .listen
        .clone()
        .unwrap_or_else(|| directory.join("control.sock"));
    #[cfg(windows)]
    let endpoint = cli
        .listen
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!(r"\\.\pipe\madrillo-{}", std::process::id())));
    let mut connections = Connections::bind(Some(&endpoint))?;
    if let Some(initial) = initial {
        connections.initial(initial, cli.stdin)?;
    }
    let mut result = Completion {
        version: PROTOCOL_VERSION,
        leader: LeaderResult::Unknown,
        cause: CompletionCause::LaunchFailed,
        exited_at_unix_ms: 0,
        cleanup: CleanupResult::Unknown,
        oom: OomEvidence::Unavailable {
            reason: "workload did not reach OOM observation".into(),
        },
        log_errors: Vec::new(),
        monitor_errors: Vec::new(),
        output_truncated: false,
    };
    let supervised = supervise(&cli, &mut connections, &mut result);
    if let Err(error) = &supervised {
        result.monitor_errors.push(error.to_string());
        if result.cause == CompletionCause::LaunchFailed {
            result.leader = LeaderResult::SpawnFailed {
                message: error.to_string(),
            };
        }
        result.exited_at_unix_ms = unix_ms();
    }
    connections.close_listener();
    state.publish(&result)?;
    connections.broadcast(ServerMessage::Completed { result })?;
    let deadline = Instant::now() + Duration::from_secs(1);
    while connections.pending() && Instant::now() < deadline {
        connections.poll()?;
        thread::sleep(Duration::from_millis(2));
    }
    connections.finish()?;
    supervised
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

fn supervise(
    cli: &Cli,
    connections: &mut Connections,
    result: &mut Completion,
) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(unix)]
    let mut signals = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGHUP,
        signal_hook::consts::SIGQUIT,
    ])?;
    let directory = cli.state_dir.as_ref().ok_or("missing state directory")?;
    let mut path = cli
        .log_path
        .is_empty()
        .then(|| directory.join("console.log"));
    let mut use_journal = false;
    let mut passthrough = false;
    for value in &cli.log_path {
        match value.to_str() {
            Some("none" | "off" | "null") => {}
            Some("passthrough") => passthrough = true,
            Some("journald") => use_journal = true,
            _ => path = Some(log_file_path(value)?),
        }
    }
    if passthrough && (path.is_some() || use_journal) {
        return Err("passthrough cannot be combined with captured logging".into());
    }
    if !use_journal
        && (cli.log_id.is_some()
            || cli.log_name.is_some()
            || cli.log_tag.is_some()
            || !cli.log_label.is_empty()
            || cli.no_container_partial_message)
    {
        return Err("journal metadata options require --log-path journald".into());
    }
    let journal = if use_journal {
        Some(logging::journal::Journal::open(
            logging::journal::Metadata {
                id: cli.log_id.as_deref().ok_or("journald requires --log-id")?,
                name: cli.log_name.as_deref(),
                tag: cli.log_tag.as_deref(),
                labels: &cli.log_label,
                no_partial: cli.no_container_partial_message,
            },
        )?)
    } else {
        None
    };
    if cli.terminal && passthrough {
        return Err("terminal capture cannot use passthrough logging".into());
    }
    let mut terminal = cli
        .terminal
        .then(|| terminal::Terminal::open(cli.size.0, cli.size.1))
        .transpose()?;
    let log = if let Some(path) = path {
        Some(FileLog::open(
            &path,
            Limits {
                size: cli.log_size_max,
                total: cli.log_global_size_max,
                backups: cli.log_rotate.then_some(cli.log_max_files),
            },
            !cli.no_sync_log,
        )?)
    } else {
        None
    };
    #[cfg(target_os = "linux")]
    let group = cli
        .cgroup
        .as_deref()
        .map(cgroup::Cgroup::open)
        .transpose()?;
    #[cfg(not(target_os = "linux"))]
    if cli.cgroup.is_some() {
        return Err("--cgroup requires Linux".into());
    }
    #[cfg(unix)]
    let mut command = launch::command(&cli.command, cli.terminal, cli.cgroup.is_some())?;
    #[cfg(windows)]
    let mut command = {
        let (program, args) = cli.command.split_first().ok_or("missing command")?;
        let mut command = std::process::Command::new(program);
        command.args(args);
        command
    };
    if let Some(cwd) = &cli.cwd {
        command.current_dir(cwd);
    }
    let mut input = if let Some(terminal) = &terminal {
        #[cfg(unix)]
        {
            Input::terminal(terminal.writer()?)
        }
        #[cfg(windows)]
        {
            let _ = terminal;
            Input::disabled()
        }
    } else if passthrough {
        command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        Input::disabled()
    } else {
        let (input, stdin) = Input::pipe()?;
        command
            .stdin(stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        input
    };
    connections.send(
        0,
        ServerMessage::Prepared {
            monitor_pid: std::process::id(),
        },
    )?;
    if cli.start_gated
        && !await_start(
            connections,
            #[cfg(unix)]
            &mut signals,
        )?
    {
        result.cause = CompletionCause::LaunchCancelled;
        result.cleanup = CleanupResult::Complete;
        result.exited_at_unix_ms = unix_ms();
        return Ok(());
    }
    let sources = if passthrough {
        0
    } else if cli.terminal {
        1
    } else {
        2
    };
    let capture = Capture::start(log, journal, !cli.no_sync_log, sources)?;
    let mut child = Workload::spawn(
        command,
        terminal.as_mut(),
        #[cfg(target_os = "linux")]
        group,
    )?;
    result.cause = CompletionCause::MonitorFailure;
    let started = Instant::now();
    // Keep the sole console writer alive with zero clients.
    let workers = (|| -> io::Result<()> {
        connections
            .broadcast(ServerMessage::Started {
                leader_pid: child.id(),
            })
            .map_err(io::Error::other)?;
        if let Some(terminal) = &mut terminal {
            #[cfg(windows)]
            {
                input = Input::terminal_windows(terminal.writer()?)?;
            }
            capture.read(terminal.reader()?, Stream::Stdout)?;
        }
        if let Some(stdout) = child.stdout() {
            capture.read(stdout, Stream::Stdout)?;
        }
        if let Some(stderr) = child.stderr() {
            capture.read(stderr, Stream::Stderr)?;
        }
        Ok(())
    })();
    let mut cleanup_errors = Vec::new();
    if let Err(error) = workers {
        if let Err(kill) = child.force() {
            cleanup_errors.push(kill.to_string());
        }
        (result.leader, result.cleanup, result.oom) = child.finish(cleanup_errors);
        return Err(error.into());
    }
    loop {
        #[cfg(unix)]
        for signal in signals.pending() {
            if let Err(error) = child.signal(signal)
                && cleanup_errors.len() < 32
            {
                cleanup_errors.push(format!("forward signal {signal}: {error}"));
            }
        }
        match service(connections, &capture, &mut input, terminal.as_ref()) {
            Ok(true) => {
                result.cause = CompletionCause::Forced;
                break;
            }
            Ok(false) => {}
            Err(error) => {
                cleanup_errors.push(format!("control service: {error}"));
                break;
            }
        }
        match child.exited() {
            Ok(true) => {
                result.cause = CompletionCause::LeaderExit;
                break;
            }
            Ok(false) => {}
            Err(error) => {
                cleanup_errors.push(format!("observe leader: {error}"));
                break;
            }
        }
        if cli
            .timeout
            .is_some_and(|seconds| started.elapsed() >= Duration::from_secs(seconds))
        {
            result.cause = CompletionCause::TimedOut;
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    result.exited_at_unix_ms = unix_ms();
    if let Err(error) = child.force() {
        cleanup_errors.push(format!("force workload: {error}"));
    }
    (result.leader, result.cleanup, result.oom) = child.finish(cleanup_errors);
    input.close();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Err(error) = service(connections, &capture, &mut input, terminal.as_ref()) {
            result.monitor_errors.push(error.to_string());
            result.output_truncated = true;
            break;
        }
        match capture.done.try_recv() {
            Ok(report) => {
                while let Ok((stream, bytes)) = capture.output.try_recv() {
                    connections.output(stream.name(), bytes)?;
                }
                result.log_errors.extend(report.errors);
                break;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(2))
            }
            Err(error) => {
                result.output_truncated = true;
                result.log_errors.push(format!(
                    "output drain/log synchronization did not complete: {error}"
                ));
                break;
            }
        }
    }
    Ok(())
}

fn await_start(
    connections: &mut Connections,
    #[cfg(unix)] signals: &mut signal_hook::iterator::Signals,
) -> Result<bool, Box<dyn std::error::Error>> {
    loop {
        #[cfg(unix)]
        if signals.pending().next().is_some() {
            return Ok(false);
        }
        for (id, message) in connections.poll()? {
            match message {
                ClientMessage::Start if id == 0 => return Ok(true),
                ClientMessage::Attach { input, streams } => {
                    connections.attach(id, input, streams)?
                }
                _ => connections.error(
                    id,
                    ErrorCode::NotRunning,
                    "workload awaits Start from its initial channel",
                )?,
            }
        }
        if !connections.initial_alive() {
            return Ok(false);
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn service(
    connections: &mut Connections,
    capture: &Capture,
    input: &mut Input,
    terminal: Option<&terminal::Terminal>,
) -> Result<bool, Box<dyn std::error::Error>> {
    let mut force = false;
    for (id, message) in connections.poll()? {
        match message {
            ClientMessage::Attach { input, streams } => connections.attach(id, input, streams)?,
            ClientMessage::Input { bytes } if connections.owns_input(id) => {
                if let Err(error) = input.enqueue(&bytes) {
                    let code = if error.kind() == io::ErrorKind::WouldBlock {
                        ErrorCode::ResourceLimit
                    } else {
                        ErrorCode::InputClosed
                    };
                    connections.error(id, code, &error.to_string())?;
                }
            }
            ClientMessage::InputEnd => connections.release_input(id),
            ClientMessage::CloseInput if terminal.is_some() => connections.error(
                id,
                ErrorCode::Unsupported,
                "a terminal has no input half-close; send terminal EOF bytes instead",
            )?,
            ClientMessage::CloseInput if connections.owns_input(id) => input.close_after_flush(),
            ClientMessage::Input { .. } | ClientMessage::CloseInput => connections.error(
                id,
                ErrorCode::InputOwned,
                "attach with input ownership first",
            )?,
            ClientMessage::Terminate => force = true,
            ClientMessage::Wait => {}
            ClientMessage::ReopenLogs => {
                if let Err(error) = capture.reopen(id) {
                    connections.error(id, ErrorCode::ResourceLimit, &error.to_string())?;
                }
            }
            ClientMessage::Resize { rows, cols } => {
                if let Some(terminal) = terminal {
                    if let Err(error) = terminal.resize(rows, cols) {
                        connections.error(id, ErrorCode::Io, &error.to_string())?;
                    }
                } else {
                    connections.error(id, ErrorCode::Unsupported, "workload has no terminal")?;
                }
            }
            _ => connections.error(id, ErrorCode::Unsupported, "request is not implemented yet")?,
        }
    }
    for _ in 0..32 {
        match capture.controls.try_recv() {
            Ok((id, Ok(()))) => connections.send(id, ServerMessage::LogsReopened)?,
            Ok((id, Err(message))) => connections.error(id, ErrorCode::Io, &message)?,
            Err(_) => break,
        }
    }
    if let Err(error) = input.flush() {
        connections.broadcast(ServerMessage::Error {
            code: ErrorCode::InputClosed,
            message: error.to_string(),
        })?;
    }
    for _ in 0..16 {
        match capture.output.try_recv() {
            Ok((stream, bytes)) => connections.output(stream.name(), bytes)?,
            Err(_) => break,
        }
    }
    Ok(force)
}
