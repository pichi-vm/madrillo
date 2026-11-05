#![forbid(unsafe_code)]
mod connections;
mod framing;
mod input;
mod logging;
mod process;
mod state;
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
    process::{Command, ExitCode, Stdio},
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
    /// CRI file path, 'none', or 'passthrough'. Defaults to STATE_DIR/console.log.
    #[arg(long)]
    log_path: Option<OsString>,
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
    /// Force the workload scope after this many seconds.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    timeout: Option<u64>,
    /// Program and native arguments, following --.
    #[arg(last = true, required_unless_present = "api_version")]
    command: Vec<OsString>,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr().lock(), "madrillo: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    if cli.api_version {
        writeln!(io::stdout().lock(), "{PROTOCOL_VERSION}")?;
        return Ok(());
    }
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
    let mut result = Completion {
        version: PROTOCOL_VERSION,
        leader: LeaderResult::Unknown,
        cause: CompletionCause::LaunchFailed,
        exited_at_unix_ms: 0,
        cleanup: CleanupResult::Unknown,
        oom: OomEvidence::Unavailable {
            reason: "OOM observation is not implemented yet".into(),
        },
        log_errors: Vec::new(),
        output_truncated: false,
    };
    if let Err(error) = supervise(&cli, &mut connections, &mut result) {
        result.log_errors.push(error.to_string());
        if result.cause == CompletionCause::LaunchFailed {
            result.leader = LeaderResult::SpawnFailed {
                message: error.to_string(),
            };
        }
        result.exited_at_unix_ms = unix_ms();
        state.publish(&result)?;
        return Err(error);
    }
    state.publish(&result)?;
    connections.broadcast(ServerMessage::Completed { result })?;
    let deadline = Instant::now() + Duration::from_secs(1);
    while connections.pending() && Instant::now() < deadline {
        connections.poll()?;
        thread::sleep(Duration::from_millis(2));
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

fn supervise(
    cli: &Cli,
    connections: &mut Connections,
    result: &mut Completion,
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = cli.state_dir.as_ref().ok_or("missing state directory")?;
    let passthrough = cli.log_path.as_deref() == Some(std::ffi::OsStr::new("passthrough"));
    let no_log = cli.log_path.as_deref() == Some(std::ffi::OsStr::new("none"));
    let log = if passthrough || no_log {
        None
    } else {
        let path = cli
            .log_path
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| directory.join("console.log"));
        Some(FileLog::open(
            &path,
            Limits {
                size: cli.log_size_max,
                total: cli.log_global_size_max,
                backups: cli.log_rotate.then_some(cli.log_max_files),
            },
        )?)
    };
    let (program, args) = cli.command.split_first().ok_or("missing command")?;
    let mut command = Command::new(program);
    command.args(args);
    if let Some(cwd) = &cli.cwd {
        command.current_dir(cwd);
    }
    if passthrough {
        command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
    } else {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
    }
    let capture = Capture::start(log, !cli.no_sync_log, if passthrough { 0 } else { 2 })?;
    let mut child = Workload::spawn(command)?;
    result.cause = CompletionCause::MonitorFailure;
    let started = Instant::now();
    // Keep the sole console writer alive with zero clients.
    let mut input = match Input::new(child.stdin()) {
        Ok(input) => input,
        Err(error) => {
            let _ = child.force();
            let _ = child.reap();
            return Err(error.into());
        }
    };
    let workers = (|| -> io::Result<()> {
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
        let _ = child.reap();
        result.cleanup = child.cleanup_result(cleanup_errors);
        return Err(error.into());
    }
    loop {
        match service(connections, &capture, &mut input) {
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
    result.leader = match child.reap() {
        Ok(status) => status,
        Err(error) => {
            cleanup_errors.push(format!("reap leader: {error}"));
            LeaderResult::Unknown
        }
    };
    result.cleanup = child.cleanup_result(cleanup_errors);
    input.close();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Err(error) = service(connections, &capture, &mut input) {
            result.log_errors.push(error.to_string());
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

fn service(
    connections: &mut Connections,
    capture: &Capture,
    input: &mut Input,
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
            ClientMessage::CloseInput if connections.owns_input(id) => input.close_after_flush(),
            ClientMessage::Input { .. } | ClientMessage::CloseInput => connections.error(
                id,
                ErrorCode::InputOwned,
                "attach with input ownership first",
            )?,
            ClientMessage::Terminate => force = true,
            ClientMessage::Wait => {}
            _ => connections.error(id, ErrorCode::Unsupported, "request is not implemented yet")?,
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
