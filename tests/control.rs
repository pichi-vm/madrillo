#![forbid(unsafe_code)]
#[path = "../src/transport.rs"]
mod transport;
use interprocess::local_socket::{GenericFilePath, Stream, prelude::*};
use madrillo::{
    ClientMessage, CompletionCause, ErrorCode, PROTOCOL_VERSION, Request, Response, ServerMessage,
};
use std::{
    io::{self, Read, Write},
    path::PathBuf,
    process::{Child, Command},
    time::{Duration, Instant},
};

#[path = "control/caller_lifetime.rs"]
mod caller_lifetime;

struct Monitor {
    child: MonitorChild,
    endpoint: PathBuf,
    _root: tempfile::TempDir,
}
impl Monitor {
    fn initial(mode: &str, gated: bool) -> Result<(Self, Client), Box<dyn std::error::Error>> {
        Self::initial_with_flags(mode, gated, &[])
    }
    fn initial_with_flags(
        mode: &str,
        gated: bool,
        flags: &[&str],
    ) -> Result<(Self, Client), Box<dyn std::error::Error>> {
        Self::initial_command(
            &[env!("CARGO_BIN_EXE_madrillo-fixture"), mode],
            gated,
            flags,
        )
    }
    #[cfg(unix)]
    fn initial_command(
        workload: &[&str],
        gated: bool,
        flags: &[&str],
    ) -> Result<(Self, Client), Box<dyn std::error::Error>> {
        use command_fds::{CommandFdExt, FdMapping};
        let root = tempfile::tempdir()?;
        let endpoint = root.path().join("control.sock");
        let (parent, initial) = std::os::unix::net::UnixStream::pair()?;
        let mut command = Command::new(env!("CARGO_BIN_EXE_madrillo"));
        command
            .arg("--state-dir")
            .arg(root.path().join("state"))
            .arg("--listen")
            .arg(&endpoint)
            .args(flags)
            .args(["--control", "22", "--timeout", "5"]);
        if gated {
            command.arg("--start-gated");
        }
        command.arg("--").args(workload);
        command.fd_mappings(vec![FdMapping {
            parent_fd: initial.into(),
            child_fd: 22,
        }])?;
        let child = command.spawn()?;
        drop(command);
        parent.set_nonblocking(true)?;
        let stream = transport::Transport::new(
            interprocess::os::unix::uds_local_socket::Stream::from(parent).into(),
        )?;
        Ok((
            Self {
                child: MonitorChild::Standard(child),
                endpoint,
                _root: root,
            },
            Client {
                stream,
                pending: Vec::new(),
            },
        ))
    }
    #[cfg(windows)]
    fn initial_command(
        workload: &[&str],
        gated: bool,
        flags: &[&str],
    ) -> Result<(Self, Client), Box<dyn std::error::Error>> {
        use interprocess::os::windows::named_pipe::{
            DuplexPipeStream, PipeListenerOptions, pipe_mode::Bytes,
        };
        let root = tempfile::tempdir()?;
        let suffix = root
            .path()
            .file_name()
            .ok_or("missing temporary name")?
            .to_string_lossy();
        let path_text = format!(r"\\.\pipe\madrillo-initial-{}-{suffix}", std::process::id());
        let endpoint = PathBuf::from(format!(
            r"\\.\pipe\madrillo-later-{}-{suffix}",
            std::process::id()
        ));
        let path = widestring::U16CString::from_str(&path_text)?;
        let listener = PipeListenerOptions::new()
            .path(path.as_ucstr())
            .nonblocking(true)
            .input_buffer_size_hint(64 * 1024)
            .output_buffer_size_hint(64 * 1024)
            .create::<Bytes, Bytes>()?;
        let initial = DuplexPipeStream::<Bytes>::connect_by_path(path_text.as_str())?;
        let parent: interprocess::os::windows::named_pipe::local_socket::Stream =
            listener.accept()?.into();
        let mut command = windows_spawn::Command::new(env!("CARGO_BIN_EXE_madrillo"));
        command
            .arg("--state-dir")
            .arg(root.path().join("state"))
            .arg("--listen")
            .arg(&endpoint)
            .args(flags)
            .arg("--control")
            .arg_handle(&initial)?
            .args(["--timeout", "5"]);
        if gated {
            command.arg("--start-gated");
        }
        command.arg("--").args(workload);
        let child = command.spawn()?;
        drop(command);
        drop(initial);
        parent.set_nonblocking(true)?;
        Ok((
            Self {
                child: MonitorChild::Inherited(child),
                endpoint,
                _root: root,
            },
            Client {
                stream: transport::Transport::new(parent.into())?,
                pending: Vec::new(),
            },
        ))
    }
    fn spawn(mode: &str) -> Result<Self, Box<dyn std::error::Error>> {
        Self::spawn_with_flags(mode, &[])
    }
    fn spawn_with_flags(mode: &str, flags: &[&str]) -> Result<Self, Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        #[cfg(unix)]
        let endpoint = root.path().join("control.sock");
        #[cfg(windows)]
        let endpoint = PathBuf::from(format!(
            r"\\.\pipe\madrillo-test-{}-{}",
            std::process::id(),
            root.path()
                .file_name()
                .ok_or("missing temporary name")?
                .to_string_lossy()
        ));
        let child = Command::new(env!("CARGO_BIN_EXE_madrillo"))
            .arg("--state-dir")
            .arg(root.path().join("state"))
            .arg("--listen")
            .arg(&endpoint)
            .args(["--timeout", "5", "--log-global-size-max", "1024"])
            .args(flags)
            .arg("--")
            .arg(env!("CARGO_BIN_EXE_madrillo-fixture"))
            .arg(mode)
            .spawn()?;
        Ok(Self {
            child: MonitorChild::Standard(child),
            endpoint,
            _root: root,
        })
    }
    fn connect(&self) -> Result<Client, Box<dyn std::error::Error>> {
        Client::connect(&self.endpoint)
    }
    fn wait(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let end = Instant::now() + Duration::from_secs(8);
        loop {
            if let Some(status) = self.child.try_wait()? {
                assert!(status.success());
                return Ok(());
            }
            if Instant::now() >= end {
                return Err("monitor test deadline".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[test]
fn terminal_size_resize_reattach_and_native_exit() -> Result<(), Box<dyn std::error::Error>> {
    let (mut monitor, mut client) = Monitor::initial_with_flags(
        "terminal-probe",
        false,
        &["--terminal", "--size", "31x99", "--stdin"],
    )?;
    assert!(matches!(client.next()?, ServerMessage::Prepared { .. }));
    assert!(matches!(client.next()?, ServerMessage::Started { .. }));
    #[cfg(unix)]
    let (size_line, exit_line) = (b"size\n", b"exit\n");
    #[cfg(windows)]
    let (size_line, exit_line) = (b"size\r", b"exit\r");
    for (rows, cols) in [(31, 99), (42, 107)] {
        client.send(ClientMessage::Resize { rows, cols })?;
        client.send(ClientMessage::Input {
            bytes: size_line.to_vec(),
        })?;
        let expected = format!("SIZE={rows}x{cols}");
        let mut text = Vec::new();
        while !String::from_utf8_lossy(&text).contains(&expected) {
            match client.next()? {
                ServerMessage::Output { stream, bytes } => {
                    assert_eq!(stream, "stdout");
                    text.extend_from_slice(&bytes);
                }
                response => return Err(format!("unexpected {response:?}").into()),
            }
        }
    }
    client.send(ClientMessage::CloseInput)?;
    assert!(matches!(
        client.next()?,
        ServerMessage::Error {
            code: ErrorCode::Unsupported,
            ..
        }
    ));
    drop(client);
    std::thread::sleep(Duration::from_millis(30));
    let mut client = monitor.connect()?;
    client.attach(true)?;
    client.send(ClientMessage::Input {
        bytes: exit_line.to_vec(),
    })?;
    loop {
        match client.next()? {
            ServerMessage::Output { .. } => {}
            ServerMessage::Completed { result } => {
                assert_eq!(result.leader, madrillo::LeaderResult::Exit { code: 37 });
                assert!(result.log_errors.is_empty());
                break;
            }
            response => return Err(format!("unexpected {response:?}").into()),
        }
    }
    monitor.wait()
}

#[cfg(unix)]
#[test]
fn stopped_workload_can_resume_without_being_finalized() -> Result<(), Box<dyn std::error::Error>> {
    use nix::{
        sys::signal::{Signal, kill},
        unistd::Pid,
    };
    let (mut monitor, mut client) = Monitor::initial_with_flags("echo", false, &["--stdin"])?;
    assert!(matches!(client.next()?, ServerMessage::Prepared { .. }));
    let ServerMessage::Started { leader_pid } = client.next()? else {
        return Err("workload was not started".into());
    };
    let pid = Pid::from_raw(i32::try_from(leader_pid)?);
    client.send(ClientMessage::Input {
        bytes: b"before stop\n".to_vec(),
    })?;
    client.output_until(b"before stop\n")?;
    kill(pid, Signal::SIGSTOP)?;
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        monitor.child.try_wait()?.is_none(),
        "a stopped workload is still alive"
    );
    assert!(!monitor._root.path().join("state/result.json").exists());
    kill(pid, Signal::SIGCONT)?;
    client.send(ClientMessage::Input {
        bytes: b"after resume\n".to_vec(),
    })?;
    client.output_until(b"after resume\n")?;
    client.send(ClientMessage::CloseInput)?;
    assert!(matches!(
        client.next()?,
        ServerMessage::Completed { result }
            if result.leader == madrillo::LeaderResult::Exit { code: 0 }
                && result.cause == CompletionCause::LeaderExit
    ));
    monitor.wait()
}

#[cfg(unix)]
#[test]
fn monitor_sigterm_is_forwarded_and_result_is_finalized() -> Result<(), Box<dyn std::error::Error>>
{
    let (mut monitor, mut client) = Monitor::initial("sleep", false)?;
    assert!(matches!(client.next()?, ServerMessage::Prepared { .. }));
    assert!(matches!(client.next()?, ServerMessage::Started { .. }));
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(i32::try_from(monitor.child.id())?),
        nix::sys::signal::Signal::SIGTERM,
    )?;
    assert!(
        matches!(client.next()?, ServerMessage::Completed { result } if result.leader == madrillo::LeaderResult::Signal { signal: 15, core_dumped: false } && result.log_errors.is_empty())
    );
    monitor.wait()
}

#[cfg(unix)]
#[test]
fn monitor_termination_signals_preserve_workload_handlers_and_logs()
-> Result<(), Box<dyn std::error::Error>> {
    use nix::sys::signal::{Signal, kill};
    for signal in [
        Signal::SIGINT,
        Signal::SIGTERM,
        Signal::SIGHUP,
        Signal::SIGQUIT,
    ] {
        let (mut monitor, mut client) = Monitor::initial("report-signal", false)?;
        assert!(matches!(client.next()?, ServerMessage::Prepared { .. }));
        assert!(matches!(client.next()?, ServerMessage::Started { .. }));
        client.output_until(b"ready")?;
        kill(
            nix::unistd::Pid::from_raw(i32::try_from(monitor.child.id())?),
            signal,
        )?;
        let expected = format!("signal={}", signal as i32);
        client.output_until(expected.as_bytes())?;
        assert!(matches!(client.next()?, ServerMessage::Completed { result }
            if result.leader == (madrillo::LeaderResult::Exit { code: 37 })
                && result.cause == CompletionCause::LeaderExit
                && result.log_errors.is_empty() && result.monitor_errors.is_empty()));
        monitor.wait()?;
        let log = std::fs::read_to_string(monitor._root.path().join("state/console.log"))?;
        assert!(log.contains(&expected), "{log}");
    }
    Ok(())
}

#[test]
fn initial_connection_retains_first_output_without_waiting_for_a_handshake()
-> Result<(), Box<dyn std::error::Error>> {
    let (mut monitor, mut client) = Monitor::initial("emit", false)?;
    // Send nothing. The inherited channel already supplies the initial
    // attachment before the process is allowed to emit a single byte.
    assert!(matches!(client.next()?, ServerMessage::Prepared { .. }));
    assert!(matches!(client.next()?, ServerMessage::Started { .. }));
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    loop {
        match client.next()? {
            ServerMessage::Output { stream, bytes } if stream == "stdout" => {
                stdout.extend_from_slice(&bytes)
            }
            ServerMessage::Output { stream, bytes } if stream == "stderr" => {
                stderr.extend_from_slice(&bytes)
            }
            ServerMessage::Completed { result } => {
                assert_eq!(result.leader, madrillo::LeaderResult::Exit { code: 37 });
                let retained: madrillo::Completion = serde_json::from_slice(&std::fs::read(
                    monitor._root.path().join("state/result.json"),
                )?)?;
                assert_eq!(retained, result);
                #[cfg(unix)]
                assert!(!monitor.endpoint.exists());
                break;
            }
            response => return Err(format!("unexpected {response:?}").into()),
        }
    }
    assert_eq!(stdout, b"first\n\npartial\0\xff");
    assert_eq!(stderr, b"diagnostic\n");
    monitor.wait()
}

#[test]
fn pre_start_disconnect_cancels_and_start_authorizes_execution()
-> Result<(), Box<dyn std::error::Error>> {
    let (mut cancelled, mut initial) = Monitor::initial("emit", true)?;
    assert!(matches!(initial.next()?, ServerMessage::Prepared { .. }));
    assert!(std::fs::read(cancelled._root.path().join("state/console.log"))?.is_empty());
    drop(initial);
    cancelled.wait()?;
    let result: madrillo::Completion = serde_json::from_slice(&std::fs::read(
        cancelled._root.path().join("state/result.json"),
    )?)?;
    assert_eq!(result.cause, CompletionCause::LaunchCancelled);
    assert_eq!(result.leader, madrillo::LeaderResult::Unknown);
    let (mut started, mut initial) = Monitor::initial("sleep", true)?;
    assert!(matches!(initial.next()?, ServerMessage::Prepared { .. }));
    initial.send(ClientMessage::Start)?;
    assert!(matches!(initial.next()?, ServerMessage::Started { .. }));
    drop(initial);
    let mut later = started.connect()?;
    later.send(ClientMessage::Terminate)?;
    assert!(
        matches!(later.next()?, ServerMessage::Completed { result } if result.cause == CompletionCause::Forced)
    );
    started.wait()
}
impl Drop for Monitor {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Client {
    stream: transport::Transport,
    pending: Vec<u8>,
}
impl Client {
    fn connect(endpoint: &std::path::Path) -> Result<Self, Box<dyn std::error::Error>> {
        let end = Instant::now() + Duration::from_secs(3);
        loop {
            match Self::try_connect(endpoint) {
                Ok(client) => return Ok(client),
                Err(_) if Instant::now() < end => std::thread::sleep(Duration::from_millis(5)),
                Err(error) => return Err(error.into()),
            }
        }
    }
    fn try_connect(endpoint: &std::path::Path) -> io::Result<Self> {
        let stream = Stream::connect(endpoint.to_fs_name::<GenericFilePath>()?)?;
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream: transport::Transport::new(stream)?,
            pending: Vec::new(),
        })
    }
    fn output_until(&mut self, needle: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut output = Vec::new();
        while !output.windows(needle.len()).any(|window| window == needle) {
            match self.next()? {
                ServerMessage::Output { stream, bytes } => {
                    assert_eq!(stream, "stdout");
                    output.extend_from_slice(&bytes);
                    assert!(output.len() <= 64 * 1024, "unexpected shell output volume");
                }
                response => return Err(format!("unexpected shell response {response:?}").into()),
            }
        }
        Ok(output)
    }
    fn send(&mut self, message: ClientMessage) -> Result<(), Box<dyn std::error::Error>> {
        let data = serde_json::to_vec(&Request {
            version: PROTOCOL_VERSION,
            message,
        })?;
        let mut frame = u32::try_from(data.len())?.to_be_bytes().to_vec();
        frame.extend_from_slice(&data);
        self.raw(&frame)
    }
    fn raw(&mut self, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        let mut written = 0;
        let end = Instant::now() + Duration::from_secs(3);
        while written < bytes.len() && Instant::now() < end {
            match self.stream.write(&bytes[written..]) {
                Ok(count) => written += count,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.into()),
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        if written != bytes.len() {
            return Err(format!("control write deadline: {written}/{} bytes", bytes.len()).into());
        }
        Ok(())
    }
    fn next(&mut self) -> Result<ServerMessage, Box<dyn std::error::Error>> {
        let end = Instant::now() + Duration::from_secs(8);
        loop {
            if self.pending.len() >= 4 {
                let size = usize::try_from(u32::from_be_bytes(self.pending[..4].try_into()?))?;
                assert!(size <= 1024 * 1024);
                if self.pending.len() >= size + 4 {
                    let response: Response = serde_json::from_slice(&self.pending[4..size + 4])?;
                    self.pending.drain(..size + 4);
                    assert_eq!(response.version, PROTOCOL_VERSION);
                    return Ok(response.message);
                }
            }
            let mut bytes = [0; 8192];
            match self.stream.read(&mut bytes) {
                Ok(0) => return Err("control EOF before expected response".into()),
                Ok(count) => {
                    self.pending.extend_from_slice(&bytes[..count]);
                    continue;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.into()),
            }
            if Instant::now() >= end {
                return Err("control read deadline".into());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn attach(&mut self, input: bool) -> Result<(), Box<dyn std::error::Error>> {
        self.send(ClientMessage::Attach {
            input,
            streams: Vec::new(),
        })?;
        assert_eq!(self.next()?, ServerMessage::Attached { input });
        Ok(())
    }
}

#[cfg(unix)]
#[test]
fn interactive_shell_job_control_survives_detach() -> Result<(), Box<dyn std::error::Error>> {
    let (mut monitor, mut client) = Monitor::initial_command(
        &[
            "/usr/bin/env",
            "PS1=MADRILLO-PROMPT> ",
            "PS2=MADRILLO-CONTINUE> ",
            "LC_ALL=C",
            "ENV=",
            "BASH_ENV=",
            "/bin/sh",
            "-i",
        ],
        false,
        &["--terminal", "--stdin"],
    )?;
    assert!(matches!(client.next()?, ServerMessage::Prepared { .. }));
    assert!(matches!(client.next()?, ServerMessage::Started { .. }));
    client.output_until(b"MADRILLO-PROMPT> ")?;
    client.send(ClientMessage::Input {
        bytes: b"sh -c 'echo JOB_READY; exec sleep 20'\n".to_vec(),
    })?;
    // Require the child's line, not the terminal's echo of its command.
    client.output_until(b"\r\nJOB_READY\r\n")?;
    client.send(ClientMessage::Input { bytes: vec![26] })?;
    let stopped = client.output_until(b"MADRILLO-PROMPT> ")?;
    assert!(
        String::from_utf8_lossy(&stopped)
            .to_lowercase()
            .contains("stopped")
    );
    drop(client);
    std::thread::sleep(Duration::from_millis(30));
    let mut client = monitor.connect()?;
    client.attach(true)?;
    client.send(ClientMessage::Input {
        bytes: b"jobs\n".to_vec(),
    })?;
    let jobs = client.output_until(b"MADRILLO-PROMPT> ")?;
    assert!(
        String::from_utf8_lossy(&jobs)
            .to_lowercase()
            .contains("stopped")
    );
    client.send(ClientMessage::Input {
        bytes: b"fg\n".to_vec(),
    })?;
    // fg prints the resumed command after returning the terminal to its job.
    client.output_until(b"sleep 20")?;
    client.send(ClientMessage::Input { bytes: vec![3] })?;
    client.output_until(b"MADRILLO-PROMPT> ")?;
    client.send(ClientMessage::Input {
        bytes: b"exit 37\n".to_vec(),
    })?;
    loop {
        match client.next()? {
            ServerMessage::Output { .. } => {}
            ServerMessage::Completed { result } => {
                assert_eq!(result.leader, madrillo::LeaderResult::Exit { code: 37 });
                assert!(result.log_errors.is_empty());
                assert!(!result.output_truncated);
                break;
            }
            response => return Err(format!("unexpected shell response {response:?}").into()),
        }
    }
    monitor.wait()
}

#[cfg(windows)]
#[test]
fn native_command_shell_interrupt_survives_detach() -> Result<(), Box<dyn std::error::Error>> {
    let (mut monitor, mut client) =
        Monitor::initial_command(&["cmd.exe", "/d", "/q"], false, &["--terminal", "--stdin"])?;
    assert!(matches!(client.next()?, ServerMessage::Prepared { .. }));
    assert!(matches!(client.next()?, ServerMessage::Started { .. }));
    client.send(ClientMessage::Input {
        bytes: b"prompt MADRILLO-PROMPT$G\r".to_vec(),
    })?;
    client.output_until(b"MADRILLO-PROMPT>")?;
    client.send(ClientMessage::Input {
        bytes: b"ping -n 20 127.0.0.1\r".to_vec(),
    })?;
    // A reply proves the foreground program is running. TTL is unchanged by
    // the native English Windows runner's console encoding and VT decoration.
    client.output_until(b"TTL=")?;
    drop(client);
    std::thread::sleep(Duration::from_millis(30));
    let mut client = monitor.connect()?;
    client.attach(true)?;
    client.send(ClientMessage::Input { bytes: vec![3] })?;
    client.output_until(b"MADRILLO-PROMPT>")?;
    client.send(ClientMessage::Input {
        bytes: b"exit 37\r".to_vec(),
    })?;
    loop {
        match client.next()? {
            ServerMessage::Output { .. } => {}
            ServerMessage::Completed { result } => {
                assert_eq!(result.leader, madrillo::LeaderResult::Exit { code: 37 });
                assert!(result.log_errors.is_empty());
                assert!(!result.output_truncated);
                break;
            }
            response => return Err(format!("unexpected shell response {response:?}").into()),
        }
    }
    monitor.wait()
}

#[test]
fn draining_viewer_survives_a_large_startup_burst() -> Result<(), Box<dyn std::error::Error>> {
    let (mut monitor, mut client) = Monitor::initial_with_flags("burst-echo", false, &["--stdin"])?;
    assert!(matches!(client.next()?, ServerMessage::Prepared { .. }));
    assert!(matches!(client.next()?, ServerMessage::Started { .. }));
    let mut received = 0;
    while received < 256 * 1024 {
        let ServerMessage::Output { bytes, .. } = client.next()? else {
            return Err("startup output interrupted".into());
        };
        assert!(bytes.iter().all(|byte| *byte == b'x'));
        received += bytes.len();
    }
    assert_eq!(received, 256 * 1024);
    client.send(ClientMessage::Input {
        bytes: b"still attached\n".to_vec(),
    })?;
    client.output_until(b"still attached\n")?;
    client.send(ClientMessage::CloseInput)?;
    assert!(
        matches!(client.next()?, ServerMessage::Completed { result } if result.leader == madrillo::LeaderResult::Exit { code: 0 })
    );
    monitor.wait()
}

#[test]
fn force_stops_grandchildren_and_concurrent_forks() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(not(any(target_os = "linux", windows)))]
    let modes = ["waiting-tree", "racing-tree"];
    #[cfg(any(target_os = "linux", windows))]
    let modes = [
        "waiting-tree",
        "racing-tree",
        "waiting-daemon-tree",
        "racing-escaped-tree",
    ];
    for terminal in [false, true].repeat(8) {
        for mode in modes {
            let root = tempfile::tempdir()?;
            let heartbeat = root.path().join("heartbeat");
            let flags = if terminal {
                vec!["--terminal"]
            } else {
                Vec::new()
            };
            let (mut monitor, mut client) = Monitor::initial_command(
                &[
                    env!("CARGO_BIN_EXE_madrillo-fixture"),
                    mode,
                    heartbeat.to_str().ok_or("non-Unicode test path")?,
                ],
                false,
                &flags,
            )?;
            let deadline = Instant::now() + Duration::from_secs(3);
            while std::fs::metadata(&heartbeat).map_or(true, |metadata| metadata.len() == 0) {
                if Instant::now() >= deadline {
                    return Err("descendants did not start".into());
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            client.send(ClientMessage::Terminate)?;
            loop {
                match client
                    .next()
                    .map_err(|error| format!("{mode}, terminal={terminal}: {error}"))?
                {
                    ServerMessage::Prepared { .. }
                    | ServerMessage::Started { .. }
                    | ServerMessage::Output { .. } => {}
                    ServerMessage::Completed { result } => {
                        assert_eq!(result.cause, CompletionCause::Forced, "{result:?}");
                        assert_eq!(
                            result.cleanup,
                            madrillo::CleanupResult::Complete,
                            "{result:?}"
                        );
                        break;
                    }
                    response => {
                        return Err(format!("unexpected force response: {response:?}").into());
                    }
                }
            }
            monitor.wait()?;
            let before = std::fs::read(&heartbeat)?;
            std::thread::sleep(Duration::from_millis(150));
            assert_eq!(std::fs::read(&heartbeat)?, before, "{mode} survived force");
        }
    }
    Ok(())
}

#[test]
fn detach_releases_writer_without_closing_child_input() -> Result<(), Box<dyn std::error::Error>> {
    let mut monitor = Monitor::spawn("echo")?;
    let mut first = monitor.connect()?;
    first.attach(true)?;
    let payload = b"hello\0\xff\x04".to_vec();
    first.send(ClientMessage::Input {
        bytes: payload.clone(),
    })?;
    assert_eq!(
        first.next()?,
        ServerMessage::Output {
            stream: "stdout".into(),
            bytes: payload.clone()
        }
    );
    let mut second = monitor.connect()?;
    second.send(ClientMessage::Attach {
        input: true,
        streams: Vec::new(),
    })?;
    assert!(matches!(
        second.next()?,
        ServerMessage::Error {
            code: ErrorCode::InputOwned,
            ..
        }
    ));
    drop(first);
    second.attach(true)?;
    second.send(ClientMessage::Input {
        bytes: payload.clone(),
    })?;
    assert_eq!(
        second.next()?,
        ServerMessage::Output {
            stream: "stdout".into(),
            bytes: payload
        }
    );
    second.send(ClientMessage::CloseInput)?;
    assert!(
        matches!(second.next()?, ServerMessage::Completed { result } if result.cause == CompletionCause::LeaderExit)
    );
    monitor.wait()
}

#[test]
fn input_end_releases_writer_and_keeps_output_attached() -> Result<(), Box<dyn std::error::Error>> {
    let mut monitor = Monitor::spawn("echo")?;
    let mut first = monitor.connect()?;
    first.attach(true)?;
    first.send(ClientMessage::InputEnd)?;
    let mut second = monitor.connect()?;
    second.attach(true)?;
    let bytes = b"new writer\0\xff\x04".to_vec();
    second.send(ClientMessage::Input {
        bytes: bytes.clone(),
    })?;
    let expected = ServerMessage::Output {
        stream: "stdout".into(),
        bytes,
    };
    assert_eq!(first.next()?, expected);
    assert_eq!(second.next()?, expected);
    second.send(ClientMessage::CloseInput)?;
    for client in [&mut first, &mut second] {
        assert!(matches!(client.next()?, ServerMessage::Completed { result }
            if result.leader == (madrillo::LeaderResult::Exit { code: 0 })));
    }
    monitor.wait()
}

#[test]
fn observer_limit_preserves_existing_control() -> Result<(), Box<dyn std::error::Error>> {
    // Fill the endpoint while gated: this tests the connection bound without
    // spending the workload's runtime deadline on attachment setup.
    let (mut monitor, mut initial) = Monitor::initial("sleep", true)?;
    assert!(matches!(initial.next()?, ServerMessage::Prepared { .. }));
    let mut viewers = Vec::new();
    for _ in 0..64 {
        let mut viewer = monitor.connect()?;
        if viewer.attach(false).is_err() {
            break;
        }
        viewers.push(viewer);
    }
    assert!(
        viewers.len() > 1 && viewers.len() < 64,
        "{} viewers",
        viewers.len()
    );
    initial.send(ClientMessage::Start)?;
    assert!(matches!(initial.next()?, ServerMessage::Started { .. }));
    initial.send(ClientMessage::Terminate)?;
    assert!(
        matches!(initial.next()?, ServerMessage::Completed { result }
        if result.cause == CompletionCause::Forced)
    );
    drop(viewers);
    monitor.wait()
}

#[test]
fn administrative_eof_does_not_require_taking_the_active_writer()
-> Result<(), Box<dyn std::error::Error>> {
    let mut monitor = Monitor::spawn("echo")?;
    let mut writer = monitor.connect()?;
    writer.attach(true)?;
    writer.send(ClientMessage::Input {
        bytes: b"accepted\n".to_vec(),
    })?;
    assert!(
        matches!(writer.next()?, ServerMessage::Output { bytes, .. } if bytes == b"accepted\n")
    );
    let mut control = monitor.connect()?;
    control.send(ClientMessage::Input {
        bytes: b"unowned data".to_vec(),
    })?;
    assert!(matches!(
        control.next()?,
        ServerMessage::Error {
            code: ErrorCode::InputOwned,
            ..
        }
    ));
    control.send(ClientMessage::CloseInput)?;
    assert!(
        matches!(control.next()?, ServerMessage::Completed { result }
        if result.leader == (madrillo::LeaderResult::Exit { code: 0 })
            && result.cause == CompletionCause::LeaderExit)
    );
    // Keep the original data owner connected throughout the EOF transition.
    assert!(matches!(writer.next()?, ServerMessage::Completed { .. }));
    monitor.wait()
}

#[test]
fn malformed_client_does_not_stop_monitor_or_other_clients()
-> Result<(), Box<dyn std::error::Error>> {
    let mut monitor = Monitor::spawn("sleep")?;
    let mut packets = vec![
        u32::MAX.to_be_bytes().to_vec(),
        0_u32.to_be_bytes().to_vec(),
        vec![0],
        vec![0, 0, 0],
        vec![0, 0, 0, 100, b'{'],
    ];
    for json in [
        br#"{"version":65535,"message":{"type":"terminate"}}"#.to_vec(),
        br#"{"version":1,"message":{"type":"invented"}}"#.to_vec(),
        br#"{"version":1,"message":{"type":"input","bytes":[256]}}"#.to_vec(),
        vec![0xff; 64],
        [vec![b'['; 200], vec![b']'; 200]].concat(),
    ] {
        let mut packet = u32::try_from(json.len())?.to_be_bytes().to_vec();
        packet.extend(json);
        packets.push(packet);
    }
    let mut clients = Vec::new();
    for packet in packets {
        let mut bad = monitor.connect()?;
        bad.raw(&packet)?;
        // Keep incomplete frames open. The monitor must not wait for their
        // remaining bytes before serving another client's termination request.
        clients.push(bad);
    }
    let mut good = monitor.connect()?;
    good.send(ClientMessage::Terminate)?;
    assert!(
        matches!(good.next()?, ServerMessage::Completed { result } if result.cause == CompletionCause::Forced)
    );
    monitor.wait()
}

#[test]
fn monitor_crash_does_not_promise_an_exit_callback() -> Result<(), Box<dyn std::error::Error>> {
    for completed in [false, true] {
        let root = tempfile::tempdir()?;
        let marker = root.path().join("callback");
        let (mut monitor, mut client) = Monitor::initial_with_flags(
            "echo",
            false,
            &[
                "--stdin",
                "--exit-delay",
                "30",
                "--exit-command",
                env!("CARGO_BIN_EXE_madrillo-fixture"),
                "--exit-command-arg",
                "touch",
                "--exit-command-arg",
                marker.to_str().ok_or("test path is not UTF-8")?,
            ],
        )?;
        assert!(matches!(client.next()?, ServerMessage::Prepared { .. }));
        assert!(matches!(client.next()?, ServerMessage::Started { .. }));
        let payload = b"before crash\0\xff\n";
        client.send(ClientMessage::Input {
            bytes: payload.to_vec(),
        })?;
        assert!(matches!(client.next()?, ServerMessage::Output { bytes, .. } if bytes == payload));
        let published = if completed {
            client.send(ClientMessage::CloseInput)?;
            let ServerMessage::Completed { result } = client.next()? else {
                return Err("missing completion before delayed callback".into());
            };
            assert_eq!(result.leader, madrillo::LeaderResult::Exit { code: 0 });
            Some(result)
        } else {
            None
        };
        // The owned monitor handle cannot identify a reused process. An unfinished
        // echo workload exits on console EOF. A finished workload's result must
        // survive even if the monitor dies before invoking the delayed callback.
        monitor.child.kill()?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = monitor.child.try_wait()? {
                assert!(!status.success());
                break;
            }
            if Instant::now() >= deadline {
                return Err("monitor crash deadline".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let error = client
            .next()
            .err()
            .ok_or("monitor crash acknowledged completion")?;
        assert!(
            error.to_string() == "control EOF before expected response"
                || error.downcast_ref::<io::Error>().is_some_and(|error| {
                    matches!(
                        error.kind(),
                        io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
                    )
                }),
            "unexpected crash channel error: {error}"
        );
        let path = monitor._root.path().join("state/result.json");
        if let Some(published) = published {
            let retained: madrillo::Completion = serde_json::from_slice(&std::fs::read(path)?)?;
            assert_eq!(retained, published);
        } else {
            assert!(!path.exists());
        }
        assert!(!marker.exists(), "monitor crash invoked its exit callback");
    }
    Ok(())
}

#[test]
fn failed_result_publication_preserves_collision_and_skips_callback()
-> Result<(), Box<dyn std::error::Error>> {
    for directory in [false, true] {
        let callback_root = tempfile::tempdir()?;
        let marker = callback_root.path().join("callback");
        let (mut monitor, mut client) = Monitor::initial_with_flags(
            "echo",
            false,
            &[
                "--stdin",
                "--exit-command",
                env!("CARGO_BIN_EXE_madrillo-fixture"),
                "--exit-command-arg",
                "touch",
                "--exit-command-arg",
                marker.to_str().ok_or("test path is not UTF-8")?,
            ],
        )?;
        assert!(matches!(client.next()?, ServerMessage::Prepared { .. }));
        assert!(matches!(client.next()?, ServerMessage::Started { .. }));
        let state = monitor._root.path().join("state");
        let destination = state.join("result.json");
        let sentinel = if directory {
            std::fs::create_dir(&destination)?;
            destination.join("sentinel")
        } else {
            destination
        };
        std::fs::write(&sentinel, b"preexisting result must survive")?;
        client.send(ClientMessage::CloseInput)?;
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if let Some(status) = monitor.child.try_wait()? {
                assert!(!status.success(), "failed publication reported success");
                break;
            }
            if Instant::now() >= deadline {
                return Err("failed publication held the monitor alive".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let error = client
            .next()
            .err()
            .ok_or("failed publication acknowledged completion")?;
        assert!(
            error.to_string() == "control EOF before expected response"
                || error.downcast_ref::<io::Error>().is_some_and(|error| {
                    error.kind() == io::ErrorKind::BrokenPipe
                        || error.kind() == io::ErrorKind::ConnectionReset
                }),
            "unexpected final channel error: {error}"
        );
        assert_eq!(std::fs::read(sentinel)?, b"preexisting result must survive");
        assert!(
            !marker.exists(),
            "callback ran without retained publication"
        );
        let mut names = std::fs::read_dir(state)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<io::Result<Vec<_>>>()?;
        names.sort();
        assert_eq!(names, ["console.log", "result.json"]);
    }
    Ok(())
}

#[test]
fn blocked_child_input_cannot_hold_force_or_timeout() -> Result<(), Box<dyn std::error::Error>> {
    for force in [false, true] {
        let mut monitor = Monitor::spawn("sleep")?;
        let mut writer = monitor.connect()?;
        writer.attach(true)?;
        for _ in 0..8 {
            writer.send(ClientMessage::Input {
                bytes: vec![b'x'; 48 * 1024],
            })?;
        }
        assert!(matches!(
            writer.next()?,
            ServerMessage::Error {
                code: madrillo::ErrorCode::ResourceLimit,
                ..
            }
        ));
        writer.send(ClientMessage::CloseInput)?;
        let mut control = monitor.connect()?;
        control.send(if force {
            ClientMessage::Terminate
        } else {
            ClientMessage::Wait
        })?;
        let ServerMessage::Completed { result } = control.next()? else {
            return Err("blocked input prevented final completion".into());
        };
        assert_eq!(
            result.cause,
            if force {
                CompletionCause::Forced
            } else {
                CompletionCause::TimedOut
            }
        );
        assert_eq!(result.cleanup, madrillo::CleanupResult::Complete);
        monitor.wait()?;
    }
    Ok(())
}

#[test]
fn stalled_viewer_does_not_block_force_or_logging() -> Result<(), Box<dyn std::error::Error>> {
    let mut monitor = Monitor::spawn("flood")?;
    let mut slow = monitor.connect()?;
    slow.attach(false)?;
    std::thread::sleep(Duration::from_millis(300));
    let mut control = monitor.connect()?;
    control.send(ClientMessage::Terminate)?;
    assert!(
        matches!(control.next()?, ServerMessage::Completed { result } if result.cause == CompletionCause::Forced && result.log_errors.is_empty())
    );
    monitor.wait()
}

#[test]
fn close_input_drains_preceding_accepted_bytes() -> Result<(), Box<dyn std::error::Error>> {
    let mut monitor = Monitor::spawn("echo")?;
    let mut client = monitor.connect()?;
    client.attach(true)?;
    let payload = vec![b'x'; 48 * 1024];
    client.send(ClientMessage::Input {
        bytes: payload.clone(),
    })?;
    client.send(ClientMessage::CloseInput)?;
    let mut observed = Vec::new();
    loop {
        match client.next()? {
            ServerMessage::Output { bytes, .. } => observed.extend_from_slice(&bytes),
            ServerMessage::Completed { result } => {
                assert_eq!(result.cause, CompletionCause::LeaderExit);
                break;
            }
            response => return Err(format!("unexpected response {response:?}").into()),
        }
    }
    assert_eq!(observed, payload);
    monitor.wait()
}

#[test]
fn reopen_rotates_the_file_and_acknowledges_completion() -> Result<(), Box<dyn std::error::Error>> {
    let mut monitor = Monitor::spawn_with_flags("echo", &["--log-rotate"])?;
    let mut client = monitor.connect()?;
    client.attach(true)?;
    client.send(ClientMessage::Input {
        bytes: b"before\n".to_vec(),
    })?;
    assert!(matches!(client.next()?, ServerMessage::Output { .. }));
    client.send(ClientMessage::ReopenLogs)?;
    assert_eq!(client.next()?, ServerMessage::LogsReopened);
    let logs = monitor._root.path().join("state");
    assert!(std::fs::read_to_string(logs.join("console.log.1"))?.ends_with("stdout F before\n"));
    assert!(std::fs::read(logs.join("console.log"))?.is_empty());
    client.send(ClientMessage::Input {
        bytes: b"after\n".to_vec(),
    })?;
    assert!(matches!(client.next()?, ServerMessage::Output { .. }));
    client.send(ClientMessage::CloseInput)?;
    assert!(
        matches!(client.next()?, ServerMessage::Completed { result } if result.log_errors.is_empty())
    );
    monitor.wait()?;
    assert!(std::fs::read_to_string(logs.join("console.log"))?.ends_with("stdout F after\n"));
    Ok(())
}

enum MonitorChild {
    Standard(Child),
    #[cfg(windows)]
    Inherited(windows_spawn::Child),
}
impl MonitorChild {
    #[cfg(unix)]
    fn id(&self) -> u32 {
        match self {
            Self::Standard(child) => child.id(),
        }
    }
    fn try_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        match self {
            Self::Standard(child) => child.try_wait(),
            #[cfg(windows)]
            Self::Inherited(child) => child.try_wait(),
        }
    }
    fn wait(&mut self) -> io::Result<std::process::ExitStatus> {
        match self {
            Self::Standard(child) => child.wait(),
            #[cfg(windows)]
            Self::Inherited(child) => child.wait(),
        }
    }
    fn kill(&mut self) -> io::Result<()> {
        match self {
            Self::Standard(child) => child.kill(),
            #[cfg(windows)]
            Self::Inherited(child) => child.kill(),
        }
    }
}

#[cfg(windows)]
#[test]
fn endpoint_acl_grants_only_the_invoking_user() -> Result<(), Box<dyn std::error::Error>> {
    let mut monitor = Monitor::spawn("sleep")?;
    let mut client = monitor.connect()?;
    let Stream::NamedPipe(stream) = &client.stream.native;
    let descriptor = win_custody::ObjectSecurityDescriptor::query_kernel_object(stream.inner())?;
    let acl = descriptor.dacl()?;
    let user = win_custody::OwnedSid::from_string(&win_custody::current_user_sid_string()?)?;
    assert_eq!(acl.len(), 1);
    assert!(
        acl.allowed_aces()
            .all(|ace| ace.trustee_matches(&user) && ace.mask() != 0)
    );
    client.send(ClientMessage::Terminate)?;
    monitor.wait()?;
    Ok(())
}

#[cfg(windows)]
#[test]
fn state_privacy_is_inherited_before_creation() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let sid = win_custody::current_user_sid_string()?;
    let user = win_custody::OwnedSid::from_string(&sid)?;
    let private = win_custody::SecurityDescriptor::from_sddl(&format!("D:P(A;OICI;FA;;;{sid})"))?;
    private.apply_to_tree(root.path(), true)?;
    let state = root.path().join("private-state");
    let output = Command::new(env!("CARGO_BIN_EXE_madrillo"))
        .arg("--state-dir")
        .arg(&state)
        .arg("--")
        .arg(env!("CARGO_BIN_EXE_madrillo-fixture"))
        .arg("emit")
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for path in [
        &state,
        &state.join("result.json"),
        &state.join("console.log"),
    ] {
        let acl = win_custody::ObjectSecurityDescriptor::query_file(path)?.dacl()?;
        assert!(!acl.is_empty());
        assert!(acl.allowed_aces().all(|ace| ace.trustee_matches(&user)));
    }
    let public = tempfile::tempdir()?;
    win_custody::SecurityDescriptor::from_sddl("D:P(A;OICI;FA;;;WD)")?
        .apply_to_tree(public.path(), true)?;
    let rejected = public.path().join("state");
    let output = Command::new(env!("CARGO_BIN_EXE_madrillo"))
        .arg("--state-dir")
        .arg(&rejected)
        .arg("--")
        .arg(env!("CARGO_BIN_EXE_madrillo-fixture"))
        .arg("emit")
        .output()?;
    assert!(!output.status.success());
    assert!(
        !rejected.exists(),
        "untrusted parent must be rejected before creating state"
    );
    Ok(())
}
