#![forbid(unsafe_code)]
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

struct Monitor {
    child: Child,
    endpoint: PathBuf,
    _root: tempfile::TempDir,
}
impl Monitor {
    #[cfg(unix)]
    fn initial(mode: &str, gated: bool) -> Result<(Self, Client), Box<dyn std::error::Error>> {
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
            .args(["--control", "22", "--timeout", "5"]);
        if gated {
            command.arg("--start-gated");
        }
        command
            .arg("--")
            .arg(env!("CARGO_BIN_EXE_madrillo-fixture"))
            .arg(mode);
        command.fd_mappings(vec![FdMapping {
            parent_fd: initial.into(),
            child_fd: 22,
        }])?;
        let child = command.spawn()?;
        drop(command);
        parent.set_nonblocking(true)?;
        let stream = interprocess::os::unix::uds_local_socket::Stream::from(parent).into();
        Ok((
            Self {
                child,
                endpoint,
                _root: root,
            },
            Client {
                stream,
                pending: Vec::new(),
            },
        ))
    }
    fn spawn(mode: &str) -> Result<Self, Box<dyn std::error::Error>> {
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
            .args(["--timeout", "5", "--log-global-size-max", "1024", "--"])
            .arg(env!("CARGO_BIN_EXE_madrillo-fixture"))
            .arg(mode)
            .spawn()?;
        Ok(Self {
            child,
            endpoint,
            _root: root,
        })
    }
    fn connect(&self) -> Result<Client, Box<dyn std::error::Error>> {
        let end = Instant::now() + Duration::from_secs(3);
        loop {
            match Stream::connect(self.endpoint.as_path().to_fs_name::<GenericFilePath>()?) {
                Ok(stream) => {
                    stream.set_nonblocking(true)?;
                    return Ok(Client {
                        stream,
                        pending: Vec::new(),
                    });
                }
                Err(_) if Instant::now() < end => std::thread::sleep(Duration::from_millis(5)),
                Err(error) => return Err(error.into()),
            }
        }
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

#[cfg(unix)]
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
                break;
            }
            response => return Err(format!("unexpected {response:?}").into()),
        }
    }
    assert_eq!(stdout, b"first\n\npartial\0\xff");
    assert_eq!(stderr, b"diagnostic\n");
    monitor.wait()
}

#[cfg(unix)]
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
    stream: Stream,
    pending: Vec<u8>,
}
impl Client {
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
            return Err("control write deadline".into());
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
                Ok(count) => self.pending.extend_from_slice(&bytes[..count]),
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
    std::thread::sleep(Duration::from_millis(40));
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
fn malformed_client_does_not_stop_monitor_or_other_clients()
-> Result<(), Box<dyn std::error::Error>> {
    let mut monitor = Monitor::spawn("sleep")?;
    let mut bad = monitor.connect()?;
    bad.raw(&u32::MAX.to_be_bytes())?;
    let mut good = monitor.connect()?;
    good.send(ClientMessage::Terminate)?;
    assert!(
        matches!(good.next()?, ServerMessage::Completed { result } if result.cause == CompletionCause::Forced)
    );
    monitor.wait()
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
