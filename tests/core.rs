#![forbid(unsafe_code)]
use madrillo::{Completion, CompletionCause, LeaderResult};
use std::{
    ffi::OsStr,
    fs,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn run(
    root: &Path,
    mode: &str,
    flags: &[&str],
    args: &[&OsStr],
) -> Result<Completion, Box<dyn std::error::Error>> {
    let state = root.join("state");
    let mut command = Command::new(env!("CARGO_BIN_EXE_madrillo"));
    command
        .arg("--state-dir")
        .arg(&state)
        .args(flags)
        .arg("--")
        .arg(env!("CARGO_BIN_EXE_madrillo-fixture"))
        .arg(mode)
        .args(args)
        .stdin(Stdio::null());
    let mut child = command.spawn()?;
    let end = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= end {
            child.kill()?;
            child.wait()?;
            return Err("monitor exceeded test deadline".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success(), "monitor failed: {status}");
    Ok(serde_json::from_slice(&fs::read(
        state.join("result.json"),
    )?)?)
}

fn payload(path: &Path, stream: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let bytes = fs::read(path)?;
    let mut output = Vec::new();
    for record in bytes.split_inclusive(|b| *b == b'\n') {
        let parts: Vec<_> = record.splitn(4, |b| *b == b' ').collect();
        assert_eq!(parts.len(), 4);
        if parts[1] != stream {
            continue;
        }
        if parts[2] == b"P" {
            output.extend_from_slice(&parts[3][..parts[3].len() - 1]);
        } else {
            assert_eq!(parts[2], b"F");
            output.extend_from_slice(parts[3]);
        }
    }
    Ok(output)
}

#[test]
fn immediate_output_and_native_exit_are_retained() -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..12 {
        let root = tempfile::tempdir()?;
        let result = run(root.path(), "emit", &[], &[])?;
        assert_eq!(result.leader, LeaderResult::Exit { code: 37 }, "{result:?}");
        assert_eq!(result.cause, CompletionCause::LeaderExit);
        assert!(result.log_errors.is_empty(), "{:?}", result.log_errors);
        assert!(!result.output_truncated);
        assert_eq!(
            payload(&root.path().join("state/console.log"), b"stdout")?,
            b"first\n\npartial\0\xff"
        );
        assert_eq!(
            payload(&root.path().join("state/console.log"), b"stderr")?,
            b"diagnostic\n"
        );
    }
    Ok(())
}

#[test]
fn detached_timeout_preserves_reason_and_console_lifetime() -> Result<(), Box<dyn std::error::Error>>
{
    for mode in ["sleep", "stdin-survival", "flood"] {
        let root = tempfile::tempdir()?;
        let result = run(
            root.path(),
            mode,
            &["--timeout", "1", "--log-global-size-max", "1024"],
            &[],
        )?;
        assert_eq!(result.cause, CompletionCause::TimedOut);
        assert!(result.log_errors.is_empty(), "{:?}", result.log_errors);
    }
    Ok(())
}

#[test]
fn leader_exit_stops_a_descendant_in_the_native_scope() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let heartbeat = root.path().join("heartbeat");
    let result = run(root.path(), "tree", &[], &[heartbeat.as_os_str()])?;
    assert_eq!(result.leader, LeaderResult::Exit { code: 37 }, "{result:?}");
    let before = fs::read(&heartbeat)?;
    assert!(!before.is_empty());
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(
        fs::read(heartbeat)?,
        before,
        "descendant survived leader cleanup"
    );
    Ok(())
}

#[test]
fn native_arguments_are_not_shell_interpreted() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let argument = OsStr::new("spaces 'quotes' $dollars `backticks` — madrillo");
    let result = run(root.path(), "args", &[], &[argument])?;
    assert_eq!(result.leader, LeaderResult::Exit { code: 0 });
    let actual = payload(&root.path().join("state/console.log"), b"stdout")?;
    #[cfg(unix)]
    let mut expected = {
        use std::os::unix::ffi::OsStrExt;
        argument.as_bytes().to_vec()
    };
    #[cfg(windows)]
    let mut expected: Vec<_> = {
        use std::os::windows::ffi::OsStrExt;
        argument.encode_wide().flat_map(u16::to_le_bytes).collect()
    };
    expected.push(b'\n');
    assert_eq!(actual, expected);
    Ok(())
}

#[test]
fn spawn_failure_is_retained_without_fake_success() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let state = root.path().join("state");
    let status = Command::new(env!("CARGO_BIN_EXE_madrillo"))
        .arg("--state-dir")
        .arg(&state)
        .arg("--")
        .arg(root.path().join("absent-program"))
        .status()?;
    assert!(!status.success());
    let result: Completion = serde_json::from_slice(&fs::read(state.join("result.json"))?)?;
    assert!(matches!(result.leader, LeaderResult::SpawnFailed { .. }));
    assert_eq!(result.cause, CompletionCause::LaunchFailed);
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn adopted_orphans_are_reaped_while_leader_is_running() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let result = run(root.path(), "orphan-reap", &[], &[])?;
    assert_eq!(result.leader, LeaderResult::Exit { code: 0 });
    Ok(())
}

#[test]
fn terminal_leader_exit_terminates_descendants() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let heartbeat = root.path().join("heartbeat");
    let result = run(
        root.path(),
        "tree",
        &["--terminal"],
        &[heartbeat.as_os_str()],
    )?;
    assert_eq!(result.leader, LeaderResult::Exit { code: 37 }, "{result:?}");
    assert!(result.log_errors.is_empty(), "{:?}", result.log_errors);
    let before = fs::read(&heartbeat)?;
    assert!(!before.is_empty());
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(fs::read(heartbeat)?, before);
    Ok(())
}

#[test]
fn disabled_and_passthrough_log_drivers_preserve_native_execution()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let result = run(root.path(), "emit", &["--log-path", "none"], &[])?;
    assert_eq!(result.leader, LeaderResult::Exit { code: 37 });
    assert!(!root.path().join("state/console.log").exists());
    let root = tempfile::tempdir()?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_madrillo"))
        .arg("--state-dir")
        .arg(root.path().join("state"))
        .args(["--log-level", "error", "--log-path", "passthrough", "--"])
        .arg(env!("CARGO_BIN_EXE_madrillo-fixture"))
        .arg("emit")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait()? {
            assert!(status.success());
            break;
        }
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            return Err("passthrough deadline".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let output = child.wait_with_output()?;
    assert_eq!(output.stdout, b"first\n\npartial\0\xff");
    assert_eq!(output.stderr, b"diagnostic\n");
    assert!(!root.path().join("state/console.log").exists());
    Ok(())
}

#[test]
fn invalid_initial_resources_are_rejected_before_state_creation()
-> Result<(), Box<dyn std::error::Error>> {
    for number in ["0".to_owned(), "1".to_owned(), usize::MAX.to_string()] {
        let root = tempfile::tempdir()?;
        let state = root.path().join("state");
        let output = Command::new(env!("CARGO_BIN_EXE_madrillo"))
            .arg("--control")
            .arg(number)
            .arg("--state-dir")
            .arg(&state)
            .arg("--")
            .arg(env!("CARGO_BIN_EXE_madrillo-fixture"))
            .arg("emit")
            .output()?;
        assert!(!output.status.success());
        assert!(!state.exists());
        assert!(output.stdout.is_empty());
    }
    Ok(())
}

#[test]
fn exit_command_observes_final_result_and_preserves_native_arguments()
-> Result<(), Box<dyn std::error::Error>> {
    for hook_status in [0, 73] {
        let root = tempfile::tempdir()?;
        let state = root.path().join("state");
        let marker = root.path().join("hook-marker");
        let argument = OsStr::new("literal 'quotes' $values `backticks` —");
        let code = hook_status.to_string();
        let mut command = Command::new(env!("CARGO_BIN_EXE_madrillo"));
        command
            .arg("--state-dir")
            .arg(&state)
            .arg("--exit-command")
            .arg(env!("CARGO_BIN_EXE_madrillo-fixture"));
        for arg in [
            OsStr::new("exit-hook"),
            state.as_os_str(),
            marker.as_os_str(),
            OsStr::new(&code),
            argument,
        ] {
            command.arg("--exit-command-arg").arg(arg);
        }
        let started = Instant::now();
        if hook_status == 0 {
            command.args(["--exit-delay", "1"]);
        }
        command
            .arg("--")
            .arg(env!("CARGO_BIN_EXE_madrillo-fixture"))
            .arg("emit");
        let mut child = command.spawn()?;
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                child.kill()?;
                child.wait()?;
                return Err("exit command test deadline".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(status.success(), hook_status == 0);
        if hook_status == 0 {
            assert!(started.elapsed() >= Duration::from_secs(1));
        }
        let result: Completion = serde_json::from_slice(&fs::read(state.join("result.json"))?)?;
        assert_eq!(result.leader, LeaderResult::Exit { code: 37 });
        assert!(result.monitor_errors.is_empty(), "{result:?}");
        #[cfg(unix)]
        let mut expected = argument.as_encoded_bytes().to_vec();
        #[cfg(windows)]
        let mut expected = {
            use std::os::windows::ffi::OsStrExt;
            argument
                .encode_wide()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>()
        };
        expected.push(b'\n');
        assert_eq!(fs::read(marker)?, expected);
    }
    Ok(())
}

#[test]
fn native_non_unicode_arguments_environment_and_cwd_survive_both_launch_modes()
-> Result<(), Box<dyn std::error::Error>> {
    use std::ffi::OsString;
    #[cfg(unix)]
    let native = {
        use std::os::unix::ffi::OsStringExt;
        OsString::from_vec(b"native-\xff-\xfe".to_vec())
    };
    #[cfg(windows)]
    let native = {
        use std::os::windows::ffi::OsStringExt;
        OsString::from_wide(&[b'n' as u16, 0xd800, b'x' as u16, 0xdc00])
    };
    fn bytes(value: &OsStr) -> Vec<u8> {
        #[cfg(unix)]
        {
            value.as_encoded_bytes().to_vec()
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            value.encode_wide().flat_map(u16::to_le_bytes).collect()
        }
    }
    for terminal in [false, true] {
        let root = tempfile::tempdir()?;
        let state = root.path().join("state");
        let cwd = root.path().join(&native);
        let cwd = match fs::create_dir(&cwd) {
            Ok(()) => cwd,
            #[cfg(target_os = "macos")]
            Err(error) if error.raw_os_error() == Some(nix::libc::EILSEQ) => {
                // APFS rejects this filename before the monitor is launched.
                // Still exercise raw argv/environment and a native Unicode cwd.
                let cwd = root.path().join("native café working directory");
                fs::create_dir(&cwd)?;
                cwd
            }
            Err(error) => return Err(error.into()),
        };
        fs::write(cwd.join("cwd-marker"), b"correct working directory")?;
        let report = root.path().join("native.json");
        let arguments = [
            OsString::new(),
            native.clone(),
            OsString::from("trailing\\"),
            OsString::from("embedded\\\"quote"),
            OsString::from("spaces ' $ ` ;"),
        ];
        let mut command = Command::new(env!("CARGO_BIN_EXE_madrillo"));
        command
            .arg("--state-dir")
            .arg(&state)
            .arg("--cwd")
            .arg(&cwd)
            .env("MADRILLO_NATIVE_VALUE", &native);
        if terminal {
            command.arg("--terminal");
        }
        command
            .arg("--")
            .arg(env!("CARGO_BIN_EXE_madrillo-fixture"))
            .arg("context-probe")
            .arg(&report)
            .args(&arguments);
        let mut child = command.spawn()?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait()? {
                assert!(status.success());
                break;
            }
            if Instant::now() >= deadline {
                child.kill()?;
                child.wait()?;
                return Err("native context deadline".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let actual: serde_json::Value = serde_json::from_slice(&fs::read(report)?)?;
        assert_eq!(
            actual,
            serde_json::json!({
                "arguments": arguments.iter().map(|value| bytes(value)).collect::<Vec<_>>(),
                "environment": bytes(&native),
                "cwd_marker": b"correct working directory".as_slice(),
            })
        );
        let result: Completion = serde_json::from_slice(&fs::read(state.join("result.json"))?)?;
        assert_eq!(result.leader, LeaderResult::Exit { code: 37 });
    }
    Ok(())
}

#[cfg(windows)]
#[test]
fn full_windows_exit_bits_survive_pipe_and_terminal_supervision()
-> Result<(), Box<dyn std::error::Error>> {
    for flags in [&[][..], &["--terminal"][..]] {
        for code in [259_u32, 0xffff_ffff_u32, 0xc000_0005_u32, 0x8000_0000_u32] {
            let root = tempfile::tempdir()?;
            let argument = (code as i32).to_string();
            let result = run(root.path(), "exit", flags, &[OsStr::new(&argument)])?;
            assert_eq!(result.leader, LeaderResult::Exit { code });
            assert_eq!(result.cause, CompletionCause::LeaderExit);
            assert!(result.exited_at_unix_ms > 0);
            assert!(!matches!(
                result.oom,
                madrillo::OomEvidence::Observed { .. }
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn native_signals_are_distinct_from_numeric_exit_codes_and_oom()
-> Result<(), Box<dyn std::error::Error>> {
    for flags in [&[][..], &["--terminal"][..]] {
        for signal in [9, 15] {
            let root = tempfile::tempdir()?;
            let argument = signal.to_string();
            let result = run(
                root.path(),
                "exit-by-signal",
                flags,
                &[OsStr::new(&argument)],
            )?;
            assert_eq!(
                result.leader,
                LeaderResult::Signal {
                    signal,
                    core_dumped: false
                }
            );
            assert_eq!(result.cause, CompletionCause::LeaderExit);
            assert!(result.exited_at_unix_ms > 0);
            assert!(!matches!(
                result.oom,
                madrillo::OomEvidence::Observed { .. }
            ));
        }
    }
    Ok(())
}
