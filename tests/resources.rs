#![forbid(unsafe_code)]
#![cfg(feature = "qualification")]
use std::{
    fs,
    time::{Duration, Instant},
};
#[cfg(unix)]
use std::{io, process::Command};

#[test]
fn held_reservation_is_released_before_exit_command() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let state = root.path().join("state");
    let path = root.path().join("reservation");
    let marker = root.path().join("hook-marker");
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    let file = options.open(&path)?;
    #[cfg(unix)]
    file.lock()?;
    #[cfg(unix)]
    let mut command = Command::new(env!("CARGO_BIN_EXE_madrillo"));
    #[cfg(windows)]
    let mut command = windows_spawn::Command::new(env!("CARGO_BIN_EXE_madrillo"));
    command
        .arg("--state-dir")
        .arg(&state)
        .arg("--exit-command")
        .arg(env!("CARGO_BIN_EXE_madrillo-fixture"));
    for argument in [
        std::ffi::OsStr::new("held-hook"),
        state.as_os_str(),
        path.as_os_str(),
        marker.as_os_str(),
    ] {
        command.arg("--exit-command-arg").arg(argument);
    }
    command.arg("--hold");
    #[cfg(unix)]
    {
        use command_fds::{CommandFdExt, FdMapping};
        command.arg("24");
        command.fd_mappings(vec![FdMapping {
            parent_fd: file.into(),
            child_fd: 24,
        }])?;
    }
    #[cfg(windows)]
    command.arg_handle(&file)?;
    command
        .arg("--")
        .arg(env!("CARGO_BIN_EXE_madrillo-fixture"))
        .arg("held-probe")
        .arg(&path);
    let mut child = command.spawn()?;
    drop(command);
    #[cfg(windows)]
    drop(file);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait()? {
            assert!(status.success(), "monitor failed: {status:?}");
            break;
        }
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            return Err("held reservation deadline".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(fs::read(marker)?, b"released");
    Ok(())
}

#[cfg(unix)]
#[test]
fn exact_descriptors_compose_with_pipe_and_terminal_launch()
-> Result<(), Box<dyn std::error::Error>> {
    use command_fds::{CommandFdExt, FdMapping};
    use std::io::Read;
    for terminal in [false, true] {
        let root = tempfile::tempdir()?;
        let (mut reader, writer) = io::pipe()?;
        let unrelated = fs::File::open("Cargo.toml")?;
        let mut command = Command::new(env!("CARGO_BIN_EXE_madrillo"));
        command
            .arg("--state-dir")
            .arg(root.path().join("state"))
            .args(["--inherit", "21"]);
        if terminal {
            command.arg("--terminal");
        }
        command
            .arg("--")
            .arg(env!("CARGO_BIN_EXE_madrillo-fixture"))
            .args(["inherited-probe", "21"]);
        command.fd_mappings(vec![
            FdMapping {
                parent_fd: writer.into(),
                child_fd: 21,
            },
            FdMapping {
                parent_fd: unrelated.into(),
                child_fd: 22,
            },
        ])?;
        let mut child = command.spawn()?;
        drop(command);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait()? {
                assert!(status.success());
                break;
            }
            if Instant::now() >= deadline {
                child.kill()?;
                child.wait()?;
                return Err("inheritance deadline".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        assert_eq!(bytes, b"opaque\0\xff");
        let result: madrillo::Completion =
            serde_json::from_slice(&fs::read(root.path().join("state/result.json"))?)?;
        assert_eq!(result.leader, madrillo::LeaderResult::Exit { code: 37 });
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn invalid_claim_cannot_become_valid_during_duplication() -> Result<(), Box<dyn std::error::Error>>
{
    use command_fds::{CommandFdExt, FdMapping};
    let root = tempfile::tempdir()?;
    let state = root.path().join("state");
    let mut command = Command::new(env!("CARGO_BIN_EXE_madrillo"));
    command
        .arg("--state-dir")
        .arg(&state)
        .args(["--hold", "10", "--hold", "3", "--"])
        .arg(env!("CARGO_BIN_EXE_madrillo-fixture"))
        .arg("emit");
    command.fd_mappings(vec![FdMapping {
        parent_fd: fs::File::open("Cargo.toml")?.into(),
        child_fd: 10,
    }])?;
    let output = command.output()?;
    assert!(!output.status.success());
    assert!(!state.exists());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("not open at adoption"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
