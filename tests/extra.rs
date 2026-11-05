#![forbid(unsafe_code)]
#![cfg(feature = "qualification")]
use std::{
    fs,
    io::{self, Write},
    path::Path,
    time::{Duration, Instant},
};
#[cfg(unix)]
type Child = std::process::Child;
#[cfg(windows)]
type Child = windows_spawn::Child;

struct Monitor(Child);
impl Monitor {
    fn spawn(
        root: &Path,
        reader: fs::File,
        terminal: bool,
        hook: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        #[cfg(unix)]
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_madrillo"));
        #[cfg(windows)]
        let mut command = windows_spawn::Command::new(env!("CARGO_BIN_EXE_madrillo"));
        command.arg("--state-dir").arg(root.join("state"));
        command.args(["--capture", "metrics"]);
        #[cfg(unix)]
        {
            use command_fds::{CommandFdExt, FdMapping};
            command.arg("30");
            command.fd_mappings(vec![FdMapping {
                parent_fd: reader.into(),
                child_fd: 30,
            }])?;
        }
        #[cfg(windows)]
        command.arg_handle(&reader)?;
        if terminal {
            command.arg("--terminal");
        }
        if hook {
            command
                .arg("--exit-command")
                .arg(env!("CARGO_BIN_EXE_madrillo-fixture"))
                .args(["--exit-command-arg", "touch-and-wait", "--exit-command-arg"])
                .arg(root.join("hook"));
        }
        command
            .arg("--")
            .arg(env!("CARGO_BIN_EXE_madrillo-fixture"))
            .arg("emit");
        let child = command.spawn()?;
        drop(command);
        Ok(Self(child))
    }
    fn wait(&mut self) -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            if let Some(status) = self.0.try_wait()? {
                return if status.success() {
                    Ok(())
                } else {
                    Err(io::Error::other(format!("monitor failed: {status:?}")))
                };
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other("capture monitor deadline"));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for Monitor {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn pipe() -> io::Result<(fs::File, io::PipeWriter)> {
    let (reader, writer) = io::pipe()?;
    #[cfg(unix)]
    let reader: std::os::fd::OwnedFd = reader.into();
    #[cfg(windows)]
    let reader: std::os::windows::io::OwnedHandle = reader.into();
    Ok((reader.into(), writer))
}

#[test]
fn named_capture_preserves_binary_bytes_separately_from_console()
-> Result<(), Box<dyn std::error::Error>> {
    for terminal in [false, true] {
        let root = tempfile::tempdir()?;
        let (reader, mut writer) = pipe()?;
        let mut monitor = Monitor::spawn(root.path(), reader, terminal, false)?;
        let bytes: Vec<_> = (0..128 * 1024).map(|n| (n % 256) as u8).collect();
        let sent = bytes.clone();
        let write = std::thread::spawn(move || writer.write_all(&sent));
        monitor.wait()?;
        write.join().map_err(|_| "capture writer panicked")??;
        assert_eq!(
            fs::read(root.path().join("state/stream-metrics.log"))?,
            bytes
        );
        let console = fs::read(root.path().join("state/console.log"))?;
        assert!(!console.windows(7).any(|bytes| bytes == b"metrics"));
        let result: madrillo::Completion =
            serde_json::from_slice(&fs::read(root.path().join("state/result.json"))?)?;
        assert!(result.log_errors.is_empty(), "{:?}", result.log_errors);
        assert!(!result.output_truncated);
    }
    Ok(())
}

#[test]
fn capture_releases_a_still_open_pipe_before_invoking_the_hook()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let (reader, mut writer) = pipe()?;
    let mut monitor = Monitor::spawn(root.path(), reader, false, true)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !root.path().join("hook").exists() {
        if monitor.0.try_wait()?.is_some() {
            return Err("monitor exited before hook".into());
        }
        if Instant::now() >= deadline {
            return Err("capture cancellation deadline".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        monitor.0.try_wait()?.is_none(),
        "hook must still keep monitor alive"
    );
    assert_eq!(
        writer.write(b"x").err().map(|e| e.kind()),
        Some(io::ErrorKind::BrokenPipe)
    );
    let result: madrillo::Completion =
        serde_json::from_slice(&fs::read(root.path().join("state/result.json"))?)?;
    assert!(result.output_truncated);
    assert!(result.log_errors.iter().any(|s| s.contains("output drain")));
    monitor.wait()?;
    Ok(())
}

#[test]
fn invalid_names_and_nonpipe_resources_fail_before_state_creation()
-> Result<(), Box<dyn std::error::Error>> {
    for name in ["stdout", "stderr", "../escape", "", "spaces here"] {
        let root = tempfile::tempdir()?;
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_madrillo"))
            .arg("--state-dir")
            .arg(root.path().join("state"))
            .args(["--capture", name, "999999", "--"])
            .arg(env!("CARGO_BIN_EXE_madrillo-fixture"))
            .arg("emit")
            .output()?;
        assert!(!output.status.success());
        assert!(!root.path().join("state").exists());
    }
    let root = tempfile::tempdir()?;
    let mut monitor = Monitor::spawn(root.path(), fs::File::open("Cargo.toml")?, false, false)?;
    assert!(monitor.wait().is_err());
    assert!(!root.path().join("state").exists());
    Ok(())
}

#[cfg(unix)]
#[test]
fn named_stream_selection_composes_with_exact_workload_inheritance()
-> Result<(), Box<dyn std::error::Error>> {
    use command_fds::{CommandFdExt, FdMapping};
    use madrillo::{ClientMessage, ServerMessage};
    use std::io::Read;
    let root = tempfile::tempdir()?;
    let (mut client, control) = std::os::unix::net::UnixStream::pair()?;
    client.set_read_timeout(Some(Duration::from_secs(10)))?;
    let (reader, writer) = io::pipe()?;
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_madrillo"));
    command
        .arg("--state-dir")
        .arg(root.path().join("state"))
        .args([
            "--capture",
            "metrics",
            "30",
            "--inherit",
            "29",
            "--control",
            "31",
            "--start-gated",
            "--",
        ])
        .arg(env!("CARGO_BIN_EXE_madrillo-fixture"))
        .args(["inherited-probe", "29"]);
    command.fd_mappings(vec![
        FdMapping {
            parent_fd: reader.into(),
            child_fd: 30,
        },
        FdMapping {
            parent_fd: writer.into(),
            child_fd: 29,
        },
        FdMapping {
            parent_fd: control.into(),
            child_fd: 31,
        },
    ])?;
    let mut monitor = Monitor(command.spawn()?);
    drop(command);
    let send = |client: &mut std::os::unix::net::UnixStream,
                message|
     -> Result<(), Box<dyn std::error::Error>> {
        let body = serde_json::to_vec(&madrillo::Request {
            version: madrillo::PROTOCOL_VERSION,
            message,
        })?;
        client.write_all(&u32::try_from(body.len())?.to_be_bytes())?;
        client.write_all(&body)?;
        Ok(())
    };
    let next = |client: &mut std::os::unix::net::UnixStream| -> Result<ServerMessage, Box<dyn std::error::Error>> {
        let mut size = [0; 4];
        client.read_exact(&mut size)?;
        let length = usize::try_from(u32::from_be_bytes(size))?;
        if length > 1024 * 1024 { return Err("oversized response".into()); }
        let mut body = vec![0; length];
        client.read_exact(&mut body)?;
        let response: madrillo::Response = serde_json::from_slice(&body)?;
        assert_eq!(response.version, madrillo::PROTOCOL_VERSION);
        Ok(response.message)
    };
    assert!(matches!(next(&mut client)?, ServerMessage::Prepared { .. }));
    send(
        &mut client,
        ClientMessage::Attach {
            input: false,
            streams: vec!["missing".into()],
        },
    )?;
    assert!(matches!(
        next(&mut client)?,
        ServerMessage::Error {
            code: madrillo::ErrorCode::InvalidRequest,
            ..
        }
    ));
    send(
        &mut client,
        ClientMessage::Attach {
            input: false,
            streams: vec!["metrics".into()],
        },
    )?;
    assert!(matches!(next(&mut client)?, ServerMessage::Attached { .. }));
    send(&mut client, ClientMessage::Start)?;
    let mut received = Vec::new();
    loop {
        match next(&mut client)? {
            ServerMessage::Started { .. } => {}
            ServerMessage::Output { stream, bytes } => {
                assert_eq!(stream, "metrics");
                received.extend_from_slice(&bytes);
            }
            ServerMessage::Completed { result } => {
                assert_eq!(result.leader, madrillo::LeaderResult::Exit { code: 37 });
                assert!(!result.output_truncated);
                break;
            }
            other => return Err(format!("unexpected response: {other:?}").into()),
        }
    }
    monitor.wait()?;
    assert_eq!(received, b"opaque\0\xff");
    assert_eq!(
        fs::read(root.path().join("state/stream-metrics.log"))?,
        received
    );
    Ok(())
}
