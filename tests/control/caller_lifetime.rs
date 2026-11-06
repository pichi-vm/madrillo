#![forbid(unsafe_code)]

use super::{Client, ClientMessage, ErrorCode, ServerMessage};
use madrillo::{CleanupResult, CompletionCause, LeaderResult};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::{
    io::{self, Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

// The caller owns the monitor's native independence. This fixture exercises that
// documented launch contract without putting caller policy in the monitor.
#[test]
#[ignore = "internal caller fixture; invoked by caller_loss_preserves_pipe_and_terminal"]
fn caller() -> Result<()> {
    #[cfg(unix)]
    use process_wrap::std::{CommandWrap, ProcessSession};
    let root = PathBuf::from(std::env::var_os("MADRILLO_CALLER_ROOT").ok_or("root missing")?);
    let endpoint = std::env::var_os("MADRILLO_CALLER_ENDPOINT").ok_or("endpoint missing")?;
    let terminal = std::env::var("MADRILLO_CALLER_TERMINAL")? == "true";
    let attached = std::env::var("MADRILLO_CALLER_ATTACHED")? == "true";
    // The parent assigns the caller's Windows Job before authorizing launch.
    assert_ne!(io::stdin().read_line(&mut String::new())?, 0);
    let mut command = Command::new(env!("CARGO_BIN_EXE_madrillo"));
    command
        .arg("--state-dir")
        .arg(root.join("state"))
        .arg("--listen")
        .arg(&endpoint)
        .args(["--stdin", "--timeout", "45"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(root.join("monitor.err"))?);
    if terminal {
        command.arg("--terminal");
    }
    command
        .arg("--")
        .arg(env!("CARGO_BIN_EXE_madrillo-fixture"))
        .arg("lifetime-probe")
        .arg(root.join("workload-ready"));
    #[cfg(unix)]
    let mut monitor = CommandWrap::from(command).wrap(ProcessSession).spawn()?;
    #[cfg(windows)]
    let mut monitor = {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        command
            .creation_flags(DETACHED_PROCESS | CREATE_BREAKAWAY_FROM_JOB)
            .spawn()?
    };
    let _connection = if attached {
        let mut client = Client::connect(std::path::Path::new(&endpoint))?;
        client.attach(true)?;
        Some(client)
    } else {
        None
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while !root.join("workload-ready").exists() {
        assert!(
            monitor.try_wait()?.is_none(),
            "monitor exited during launch"
        );
        assert!(Instant::now() < deadline, "workload did not start");
        std::thread::sleep(Duration::from_millis(5));
    }
    std::fs::write(root.join("caller-ready"), b"ready")?;
    // Returning drops this caller's handles without stopping the independent
    // monitor. Crash/hangup cases end this process before the read returns.
    io::stdin().read_exact(&mut [0])?;
    Ok(())
}

struct Caller {
    process: Box<dyn portable_pty::Child + Send + Sync>,
    endpoint: PathBuf,
    #[cfg(windows)]
    job: Option<win32job::Job>,
}

impl Drop for Caller {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
        // Also clean the independent monitor on a failed assertion. Its own
        // bounded timeout remains the fallback if the endpoint is unavailable.
        if let Ok(mut client) = Client::try_connect(&self.endpoint) {
            let _ = client.send(ClientMessage::Terminate);
            let _ = client.next();
        }
    }
}

#[test]
#[cfg_attr(
    windows,
    ignore = "requires execution outside restrictive runner/Cargo Jobs"
)]
fn caller_loss_preserves_pipe_and_terminal() -> Result<()> {
    #[cfg(unix)]
    use nix::{
        sys::signal::{Signal, killpg},
        unistd::{Pid, getpgid},
    };
    for terminal in [false, true] {
        for attached in [false, true] {
            for loss in ["exit", "crash", "hangup"] {
                let root = tempfile::tempdir()?;
                #[cfg(unix)]
                let endpoint = root.path().join("control.sock");
                #[cfg(windows)]
                let endpoint = PathBuf::from(format!(
                    r"\\.\pipe\madrillo-caller-{}-{}",
                    std::process::id(),
                    root.path()
                        .file_name()
                        .ok_or("temporary name missing")?
                        .to_string_lossy()
                ));
                let pair = native_pty_system().openpty(PtySize::default())?;
                let mut command = CommandBuilder::new(std::env::current_exe()?);
                command.args([
                    "caller_lifetime::caller",
                    "--exact",
                    "--ignored",
                    "--nocapture",
                ]);
                command.env("MADRILLO_CALLER_ROOT", root.path());
                command.env("MADRILLO_CALLER_ENDPOINT", &endpoint);
                command.env("MADRILLO_CALLER_TERMINAL", terminal.to_string());
                command.env("MADRILLO_CALLER_ATTACHED", attached.to_string());
                let mut caller = Caller {
                    process: pair.slave.spawn_command(command)?,
                    endpoint,
                    #[cfg(windows)]
                    job: None,
                };
                #[cfg(windows)]
                {
                    let mut limits = win32job::ExtendedLimitInfo::new();
                    limits.limit_kill_on_job_close().limit_breakaway_ok();
                    let job = win32job::Job::create_with_limit_info(&limits)?;
                    job.assign_process(
                        caller
                            .process
                            .as_raw_handle()
                            .ok_or("caller handle missing")? as isize,
                    )?;
                    caller.job = Some(job);
                }
                drop(pair.slave);
                let mut writer = pair.master.take_writer()?;
                #[cfg(windows)]
                {
                    // portable-pty requests the host cursor position. Drain
                    // output throughout the fixture, and answer only after the
                    // actual request so ConPTY cannot discard an early reply.
                    let mut reader = pair.master.try_clone_reader()?;
                    let (request, cursor) = std::sync::mpsc::sync_channel(1);
                    std::thread::spawn(move || {
                        let mut tail = [0; 4];
                        let mut bytes = [0; 4096];
                        while let Ok(count) = reader.read(&mut bytes) {
                            if count == 0 {
                                break;
                            }
                            for byte in &bytes[..count] {
                                tail.rotate_left(1);
                                tail[3] = *byte;
                                if tail == *b"\x1b[6n" {
                                    let _ = request.try_send(());
                                }
                            }
                        }
                    });
                    cursor.recv_timeout(Duration::from_secs(8))?;
                    writer.write_all(b"\x1b[1;1R")?;
                }
                #[cfg(unix)]
                let newline = "\n";
                #[cfg(windows)]
                let newline = "\r";
                writer.write_all(format!("go{newline}").as_bytes())?;
                let deadline = Instant::now() + Duration::from_secs(8);
                while !root.path().join("caller-ready").exists() {
                    assert!(caller.process.try_wait()?.is_none(), "caller exited early");
                    assert!(Instant::now() < deadline, "caller launch deadline");
                    std::thread::sleep(Duration::from_millis(5));
                }
                // Closing the master tests actual terminal loss. Killing the
                // whole caller group verifies that the monitor left that scope.
                assert!(
                    caller.process.try_wait()?.is_none(),
                    "caller exited before {loss}"
                );
                match loss {
                    "exit" => writer.write_all(format!("exit{newline}").as_bytes())?,
                    "crash" => {
                        #[cfg(unix)]
                        {
                            let pid = Pid::from_raw(i32::try_from(
                                caller.process.process_id().ok_or("caller PID missing")?,
                            )?);
                            assert_eq!(getpgid(Some(pid))?, pid);
                            killpg(pid, Signal::SIGKILL)?;
                        }
                        // Closing the only Job handle kills all caller members.
                        #[cfg(windows)]
                        drop(caller.job.take());
                    }
                    "hangup" => {}
                    _ => unreachable!(),
                }
                if loss == "hangup" {
                    drop(writer);
                    drop(pair.master);
                }
                let deadline = Instant::now() + Duration::from_secs(10);
                loop {
                    if let Some(status) = caller.process.try_wait()? {
                        if loss == "exit" {
                            assert!(status.success(), "{loss}: {status:?}");
                        }
                        break;
                    }
                    assert!(Instant::now() < deadline, "caller survived {loss}");
                    std::thread::sleep(Duration::from_millis(5));
                }
                assert!(!root.path().join("state/result.json").exists());
                let mut client = Client::connect(&caller.endpoint)?;
                let deadline = Instant::now() + Duration::from_secs(3);
                loop {
                    client.send(ClientMessage::Attach {
                        input: true,
                        streams: Vec::new(),
                    })?;
                    match client.next()? {
                        ServerMessage::Attached { input: true } => break,
                        ServerMessage::Error {
                            code: ErrorCode::InputOwned,
                            ..
                        } if Instant::now() < deadline => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        response => return Err(format!("reattachment failed: {response:?}").into()),
                    }
                }
                // Pipe input has no console translation on Windows.
                let workload_newline = if cfg!(windows) && terminal {
                    "\r"
                } else {
                    "\n"
                };
                client.send(ClientMessage::Input {
                    bytes: format!("after caller loss{workload_newline}").into_bytes(),
                })?;
                // Require a workload-generated response, not the PTY's echo.
                client.output_until(b"PROBE:after caller loss")?;
                client.send(ClientMessage::Input {
                    bytes: format!("exit{workload_newline}").into_bytes(),
                })?;
                loop {
                    match client.next()? {
                        ServerMessage::Output { .. } => {}
                        ServerMessage::Completed { result } => {
                            assert_eq!(result.leader, LeaderResult::Exit { code: 37 });
                            assert_eq!(result.cause, CompletionCause::LeaderExit);
                            // C04 proves independent lifetime. Windows ConPTY's
                            // missing Job-empty evidence remains a C07 blocker.
                            if cfg!(windows) && terminal {
                                assert!(matches!(result.cleanup,
                                    CleanupResult::Incomplete { ref errors } if !errors.is_empty()));
                            } else {
                                assert_eq!(result.cleanup, CleanupResult::Complete);
                            }
                            assert!(result.monitor_errors.is_empty());
                            assert!(result.log_errors.is_empty());
                            break;
                        }
                        response => {
                            return Err(format!("unexpected completion: {response:?}").into());
                        }
                    }
                }
                eprintln!(
                    "caller lifetime: terminal={terminal}, attached={attached}, loss={loss} passed"
                );
            }
        }
    }
    Ok(())
}
