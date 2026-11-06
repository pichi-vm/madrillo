#![deny(unsafe_code)]
mod activation;
#[cfg(target_os = "linux")]
mod cgroup;
mod connections;
mod diagnostics;
mod framing;
mod input;
#[cfg(unix)]
mod launch;
mod logging;
mod notify;
#[cfg(unix)]
mod oom;
mod process;
mod resources;
#[cfg(windows)]
mod security;
mod state;
mod terminal;
mod transport;
#[cfg(windows)]
#[allow(unsafe_code, reason = "User-approved, audited Windows OS boundary")]
mod windows;
use clap::Parser;
use connections::Connections;
use diagnostics::{Diagnostics, Level};
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
    /// Working directory for the child.
    #[arg(long)]
    cwd: Option<PathBuf>,
    /// Private Unix socket or Windows named-pipe endpoint for live attachment.
    #[arg(long)]
    listen: Option<PathBuf>,
    /// Ownership of the already-connected initial control channel.
    #[arg(long)]
    control: Option<usize>,
    /// Retain this inherited resource until workload cleanup; never pass it on.
    #[arg(long)]
    hold: Vec<usize>,
    /// Pass this inherited descriptor to the workload at the same number (Unix).
    #[arg(long)]
    inherit: Vec<usize>,
    /// Capture an inherited read pipe as NAME; raw history is stream-NAME.log.
    #[arg(long, num_args = 2, value_names = ["NAME", "RESOURCE"])]
    capture: Vec<String>,
    /// Transfer systemd activation sockets even when LISTEN_PID names the caller.
    #[arg(long)]
    replace_listen_pid: bool,
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
    /// Permit file logs only below one of these existing directories; repeatable.
    #[arg(long)]
    log_allowlist_dir: Vec<PathBuf>,
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
    /// Monitor diagnostic threshold: error, warn, info, debug, or trace.
    #[arg(long, default_value = "warn")]
    log_level: Level,
    /// Also send diagnostics to syslog (Apple unified logging on macOS).
    #[cfg(unix)]
    #[arg(long)]
    syslog: bool,
    /// Relay allowed service notifications to this systemd socket (Linux).
    #[arg(long)]
    sdnotify_socket: Option<OsString>,
    /// Existing, empty, caller-owned Linux cgroup directory for this workload.
    #[arg(long)]
    cgroup: Option<PathBuf>,
    /// Force the workload scope after this many seconds.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    timeout: Option<u64>,
    /// Execute this native program after final result publication and notification.
    #[arg(long)]
    exit_command: Option<OsString>,
    /// Literal argument to the exit command; repeat for each argument.
    #[arg(long, requires = "exit_command", allow_hyphen_values = true)]
    exit_command_arg: Vec<OsString>,
    /// Delay the exit command by this many seconds.
    #[arg(long, requires = "exit_command")]
    exit_delay: Option<u64>,
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
    let cli = Cli::parse();
    let level = cli.log_level;
    #[cfg(unix)]
    let syslog = cli.syslog;
    #[cfg(not(unix))]
    let syslog = false;
    let mut diagnostics = None;
    let result = run(cli, &mut diagnostics);
    if let Err(error) = &result {
        if diagnostics.is_none() {
            diagnostics = Diagnostics::start(level, syslog).ok();
        }
        if let Some(diagnostics) = &mut diagnostics {
            diagnostics.emit(Level::Error, format_args!("{error}"));
        }
    }
    let diagnostic_errors = diagnostics
        .as_mut()
        .map(Diagnostics::flush)
        .unwrap_or_default();
    if result.is_ok() && diagnostic_errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
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

fn run(cli: Cli, diagnostics: &mut Option<Diagnostics>) -> Result<(), Box<dyn std::error::Error>> {
    if cli.api_version {
        writeln!(io::stdout().lock(), "{PROTOCOL_VERSION}")?;
        return Ok(());
    }
    // Adopt before opening state, endpoints, logs or any worker resources.
    let capture_specs = logging::extra::Spec::parse(&cli.capture)?;
    let activation = activation::Activation::discover(cli.replace_listen_pid)?;
    let mut inherited_numbers = cli.inherit.clone();
    if let Some(activation) = &activation {
        for number in activation.numbers() {
            if !inherited_numbers.contains(&number) {
                inherited_numbers.push(number);
            }
        }
    }
    let numbers: Vec<_> = cli
        .control
        .iter()
        .chain(&cli.hold)
        .chain(&inherited_numbers)
        .copied()
        .chain(capture_specs.iter().map(|spec| spec.number))
        .collect();
    let mut held = resources::adopt(&numbers)?;
    #[cfg(unix)]
    let syslog = cli.syslog;
    #[cfg(not(unix))]
    let syslog = false;
    *diagnostics = Some(Diagnostics::start(cli.log_level, syslog)?);
    let diagnostics = diagnostics
        .as_mut()
        .ok_or("diagnostics worker unavailable")?;
    diagnostics.emit(Level::Debug, format_args!("preparing workload"));
    let initial = cli
        .control
        .and_then(|number| held.remove(&number))
        .map(resources::control)
        .transpose()?;
    let extra = capture_specs
        .into_iter()
        .map(|spec| {
            let resource = held
                .remove(&spec.number)
                .ok_or_else(|| io::Error::other("capture resource disappeared"))?;
            Ok((spec.name, logging::extra::Reader::new(resource)?))
        })
        .collect::<io::Result<Vec<_>>>()?;
    #[cfg(unix)]
    let inherited = inherited_numbers
        .iter()
        .map(|number| {
            let resource = held
                .remove(number)
                .ok_or_else(|| io::Error::other("inherited resource disappeared"))?;
            Ok(command_fds::FdMapping {
                parent_fd: resource.as_file().map_err(io::Error::other)?.into(),
                child_fd: i32::try_from(*number).map_err(io::Error::other)?,
            })
        })
        .collect::<io::Result<Vec<_>>>()?;
    #[cfg(windows)]
    let inherited = inherited_numbers
        .iter()
        .map(|number| {
            held.remove(number)
                .ok_or_else(|| io::Error::other("inherited resource disappeared"))
        })
        .collect::<io::Result<Vec<_>>>()?;
    let directory = cli.state_dir.as_ref().ok_or("missing state directory")?;
    let mut state = State::create(directory)?;
    #[cfg(unix)]
    let (original_oom_score, oom_warning) = oom::protect();
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
    let mut connections = Connections::bind(
        Some(&endpoint),
        extra.iter().map(|(name, _)| name.clone()).collect(),
    )?;
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
        monitor_warnings: Vec::new(),
        output_truncated: false,
    };
    #[cfg(unix)]
    result.monitor_warnings.extend(oom_warning);
    let supervised = supervise(
        &cli,
        &mut connections,
        &mut result,
        &mut state,
        activation.as_ref(),
        extra,
        diagnostics,
        #[cfg(unix)]
        original_oom_score,
        inherited,
    );
    if let Err(error) = &supervised {
        result.monitor_errors.push(error.to_string());
        if result.cause == CompletionCause::LaunchFailed {
            result.leader = LeaderResult::SpawnFailed {
                message: error.to_string(),
            };
            result.exited_at_unix_ms = unix_ms();
        }
    }
    drop(held);
    connections.close_listener();
    if matches!(result.oom, OomEvidence::Observed { .. })
        && let Err(error) = state.retain_oom()
    {
        result
            .monitor_errors
            .push(format!("retain OOM evidence: {error}"));
    }
    if let Err(error) = state.finish_oom() {
        result
            .monitor_errors
            .push(format!("retain OOM evidence: {error}"));
    }
    for warning in &result.monitor_warnings {
        diagnostics.emit(Level::Warn, format_args!("{warning}"));
    }
    for error in result.monitor_errors.iter().chain(&result.log_errors) {
        diagnostics.emit(Level::Error, format_args!("{error}"));
    }
    diagnostics.emit(
        Level::Info,
        format_args!(
            "workload completed: {:?}; leader {:?}; cleanup {:?}",
            result.cause, result.leader, result.cleanup
        ),
    );
    result.monitor_errors.extend(diagnostics.flush());
    state.publish(&result)?;
    connections.broadcast(ServerMessage::Completed { result })?;
    let deadline = Instant::now() + Duration::from_secs(1);
    while connections.pending() && Instant::now() < deadline {
        connections.poll()?;
        thread::sleep(Duration::from_millis(2));
    }
    connections.finish()?;
    drop(connections);
    let callback = exit_command(
        &cli,
        #[cfg(unix)]
        original_oom_score,
    );
    // The callback runs for retained launch failures too. Its outcome never
    // rewrites result.json or the completion already sent to clients.
    if let Err(error) = &callback
        && supervised.is_err()
    {
        diagnostics.emit(Level::Error, format_args!("{error}"));
    }
    supervised?;
    callback
}

fn exit_command(
    cli: &Cli,
    #[cfg(unix)] original_oom_score: Option<i32>,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(program) = &cli.exit_command else {
        return Ok(());
    };
    if let Some(delay) = cli.exit_delay {
        thread::sleep(Duration::from_secs(delay));
    }
    #[cfg(target_os = "linux")]
    nix::sys::prctl::set_child_subreaper(false)?;
    #[cfg(unix)]
    let mut child = {
        let args = std::iter::once(program.clone())
            .chain(cli.exit_command_arg.iter().cloned())
            .collect::<Vec<_>>();
        let mut command = launch::command(&args, false, false, original_oom_score)?;
        // These resources belong exclusively to the original workload.
        command
            .env_remove("LISTEN_PID")
            .env_remove("LISTEN_PIDFDID")
            .env_remove("LISTEN_FDS")
            .env_remove("LISTEN_FDNAMES")
            .env_remove("NOTIFY_SOCKET");
        launch::spawn(command, None, Vec::new())?
    };
    #[cfg(windows)]
    let mut child = std::process::Command::new(program)
        .args(&cli.exit_command_arg)
        .env_remove("LISTEN_PID")
        .env_remove("LISTEN_PIDFDID")
        .env_remove("LISTEN_FDS")
        .env_remove("LISTEN_FDNAMES")
        .env_remove("NOTIFY_SOCKET")
        .spawn()?;
    let status = child.wait()?;
    if !status.success() {
        return Err(format!("exit command failed: {status}").into());
    }
    Ok(())
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

#[allow(
    clippy::too_many_arguments,
    reason = "Explicit native launch resources and supervisor-owned state"
)]
fn supervise(
    cli: &Cli,
    connections: &mut Connections,
    result: &mut Completion,
    #[cfg_attr(not(target_os = "linux"), allow(unused_variables))] state: &mut State,
    activation: Option<&activation::Activation>,
    extra: Vec<(String, logging::extra::Reader)>,
    diagnostics: &mut Diagnostics,
    #[cfg(unix)] original_oom_score: Option<i32>,
    #[cfg(unix)] inherited: Vec<command_fds::FdMapping>,
    #[cfg(windows)] inherited: Vec<filedescriptor::FileDescriptor>,
) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(unix)]
    let mut signals = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGHUP,
        signal_hook::consts::SIGQUIT,
        // Replace an inherited SIG_IGN/SA_NOCLDWAIT so children stay waitable
        // and their numeric identities cannot be reused before owned reaping.
        signal_hook::consts::SIGCHLD,
    ])?;
    let directory = cli.state_dir.as_ref().ok_or("missing state directory")?;
    let mut notify = cli
        .sdnotify_socket
        .as_deref()
        .map(|target| notify::Notify::open(directory, target))
        .transpose()?;
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
            &cli.log_allowlist_dir,
        )?)
    } else {
        None
    };
    let extra_logs = extra
        .iter()
        .map(|(name, _)| {
            let log = FileLog::open(
                &directory.join(format!("stream-{name}.log")),
                Limits {
                    size: cli.log_size_max,
                    total: cli.log_global_size_max,
                    backups: cli.log_rotate.then_some(cli.log_max_files),
                },
                !cli.no_sync_log,
                &cli.log_allowlist_dir,
            )?;
            Ok((name.clone(), log))
        })
        .collect::<io::Result<Vec<_>>>()?;
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
    let mut command = launch::command(
        &cli.command,
        cli.terminal,
        cli.cgroup.is_some(),
        original_oom_score,
    )?;
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
    activation::clear(&mut command);
    if let Some(activation) = activation {
        activation.configure(&mut command);
    }
    if let Some(notify) = &notify {
        notify.configure(&mut command);
    }
    #[cfg(windows)]
    let mut child_stdin = None;
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
        #[cfg(unix)]
        command
            .stdin(stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            child_stdin = Some(stdin);
        }
        input
    };
    connections.send(
        0,
        ServerMessage::Prepared {
            monitor_pid: std::process::id(),
        },
    )?;
    diagnostics.emit(
        Level::Trace,
        format_args!("endpoint, logs and native launch resources prepared"),
    );
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
    let sources = extra.len()
        + if passthrough {
            0
        } else if cli.terminal {
            1
        } else {
            2
        };
    let mut capture = Capture::start(log, journal, !cli.no_sync_log, sources, extra_logs)?;
    let mut child = Workload::spawn(
        command,
        terminal.as_mut(),
        #[cfg(target_os = "linux")]
        group,
        inherited,
        #[cfg(windows)]
        child_stdin,
        #[cfg(windows)]
        passthrough,
    )?;
    result.cause = CompletionCause::MonitorFailure;
    let started = Instant::now();
    diagnostics.emit(
        Level::Info,
        format_args!("workload started: leader {}", child.id()),
    );
    // Keep the sole console writer alive with zero clients.
    let workers = (|| -> io::Result<()> {
        for (index, (_, reader)) in extra.into_iter().enumerate() {
            capture.read_extra(reader, index)?;
        }
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
        (
            result.leader,
            result.cleanup,
            result.oom,
            result.exited_at_unix_ms,
        ) = child.finish(cleanup_errors);
        return Err(error.into());
    }
    #[cfg(target_os = "linux")]
    let mut observe_oom = true;
    loop {
        #[cfg(target_os = "linux")]
        if observe_oom {
            match child.poll_oom() {
                Ok(true) => {
                    observe_oom = false;
                    if let Err(error) = state.retain_oom() {
                        result
                            .monitor_errors
                            .push(format!("retain OOM evidence: {error}"));
                    }
                }
                Ok(false) => {}
                Err(error) => {
                    observe_oom = false;
                    result
                        .monitor_warnings
                        .push(format!("live OOM observation unavailable: {error}"));
                }
            }
        }
        #[cfg(unix)]
        for signal in signals
            .pending()
            .filter(|signal| *signal != signal_hook::consts::SIGCHLD)
        {
            if let Err(error) = child.signal(signal)
                && cleanup_errors.len() < 32
            {
                cleanup_errors.push(format!("forward signal {signal}: {error}"));
            }
        }
        pump_notifications(&mut notify, result);
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
    if let Err(error) = child.force() {
        cleanup_errors.push(format!("force workload: {error}"));
    }
    (
        result.leader,
        result.cleanup,
        result.oom,
        result.exited_at_unix_ms,
    ) = child.finish(cleanup_errors);
    input.close();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        pump_notifications(&mut notify, result);
        if let Err(error) = service(connections, &capture, &mut input, terminal.as_ref()) {
            result.monitor_errors.push(error.to_string());
            result.output_truncated = true;
            break;
        }
        match capture.done.try_recv() {
            Ok(report) => {
                while let Ok((stream, bytes)) = capture.output.try_recv() {
                    connections.output(&stream, bytes)?;
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
    let deadline = Instant::now() + Duration::from_secs(1);
    while !pump_notifications(&mut notify, result) {
        if Instant::now() >= deadline {
            result
                .monitor_errors
                .push("notification relay: final delivery deadline expired".into());
            break;
        }
        service(connections, &capture, &mut input, terminal.as_ref())?;
        thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}

fn await_start(
    connections: &mut Connections,
    #[cfg(unix)] signals: &mut signal_hook::iterator::Signals,
) -> Result<bool, Box<dyn std::error::Error>> {
    loop {
        #[cfg(unix)]
        if signals
            .pending()
            .any(|signal| signal != signal_hook::consts::SIGCHLD)
        {
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
            // Closing pipe input is administrative control, like Terminate.
            // All peers are authenticated; data writes still require ownership.
            ClientMessage::CloseInput => input.close_after_flush(),
            ClientMessage::Input { .. } => connections.error(
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
            Ok((stream, bytes)) => connections.output(&stream, bytes)?,
            Err(_) => break,
        }
    }
    Ok(force)
}

fn pump_notifications(notify: &mut Option<notify::Notify>, result: &mut Completion) -> bool {
    if let Some(relay) = notify {
        match relay.pump() {
            Ok(drained) => return drained,
            Err(error) => {
                result
                    .monitor_errors
                    .push(format!("notification relay: {error}"));
                *notify = None;
            }
        }
    }
    true
}
