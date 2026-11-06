#![forbid(unsafe_code)]
#![cfg(feature = "qualification")]

use madrillo::{Completion, CompletionCause, LeaderResult};
use std::{
    fs,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

fn wait(child: &mut Child) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(8);
    while child.try_wait()?.is_none() {
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            return Err("diagnostic sink blocked monitor exit".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

#[test]
fn thresholds_preserve_workload_logs_and_aliases() -> Result<(), Box<dyn std::error::Error>> {
    for (level, verbosity) in [
        (None, 0),
        (Some("error"), 0),
        (Some("FATAL"), 0),
        (Some("panic"), 0),
        (Some("WARNING"), 0),
        (Some("info"), 1),
        (Some("debug"), 2),
        (Some("TrAcE"), 3),
    ] {
        let root = tempfile::tempdir()?;
        let state = root.path().join("state");
        let mut command = Command::new(env!("CARGO_BIN_EXE_madrillo"));
        command.arg("--state-dir").arg(&state);
        if let Some(level) = level {
            command.args(["--log-level", level]);
        }
        let mut child = command
            .arg("--")
            .arg(env!("CARGO_BIN_EXE_madrillo-fixture"))
            .arg("emit")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        wait(&mut child)?;
        let output = child.wait_with_output()?;
        assert!(output.status.success(), "{output:?}");
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr)?;
        assert_eq!(stderr.contains("[Info]"), verbosity >= 1, "{stderr}");
        assert_eq!(stderr.contains("[Debug]"), verbosity >= 2, "{stderr}");
        assert_eq!(stderr.contains("[Trace]"), verbosity >= 3, "{stderr}");
        assert!(!stderr.contains("\ndiagnostic\n"));
        let log = fs::read(state.join("console.log"))?;
        assert!(
            log.windows(b"stderr F diagnostic\n".len())
                .any(|part| part == b"stderr F diagnostic\n")
        );
        assert!(
            !log.windows(b"madrillo [".len())
                .any(|part| part == b"madrillo [")
        );
        let result: Completion = serde_json::from_slice(&fs::read(state.join("result.json"))?)?;
        assert_eq!(result.leader, LeaderResult::Exit { code: 37 });
        assert!(result.monitor_errors.is_empty(), "{result:?}");
    }
    Ok(())
}

#[test]
fn fatal_errors_survive_threshold_and_invalid_levels_launch_nothing()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let state = root.path().join("state");
    let mut child = Command::new(env!("CARGO_BIN_EXE_madrillo"))
        .arg("--state-dir")
        .arg(&state)
        .args(["--log-level", "panic", "--"])
        .arg(root.path().join("absent-program"))
        .stderr(Stdio::piped())
        .spawn()?;
    wait(&mut child)?;
    let output = child.wait_with_output()?;
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr)?.contains("madrillo [Error]:"));
    let invalid_state = root.path().join("invalid");
    let output = Command::new(env!("CARGO_BIN_EXE_madrillo"))
        .arg("--state-dir")
        .arg(&invalid_state)
        .args(["--log-level", "off", "--"])
        .arg(env!("CARGO_BIN_EXE_madrillo-fixture"))
        .arg("emit")
        .output()?;
    assert!(!output.status.success());
    assert!(!invalid_state.exists());
    Ok(())
}

#[test]
fn a_full_diagnostic_sink_cannot_block_timeout_or_result_publication()
-> Result<(), Box<dyn std::error::Error>> {
    use std::{io::Write, sync::mpsc, thread};
    let root = tempfile::tempdir()?;
    let state = root.path().join("state");
    let (reader, writer) = std::io::pipe()?;
    let mut filling_writer = writer.try_clone()?;
    let (ready, started) = mpsc::sync_channel(1);
    let filler = thread::Builder::new()
        .name("fill-diagnostic-pipe".into())
        .spawn(move || -> std::io::Result<()> {
            filling_writer.write_all(b"x")?;
            let _ = ready.send(());
            loop {
                filling_writer.write_all(&[b'x'; 8192])?;
            }
        })?;
    // Use ordinary synchronous pipes on every host, including Windows. Closing
    // the unread end releases the filler even when launching the monitor fails.
    let outcome = (|| -> Result<Completion, Box<dyn std::error::Error>> {
        started.recv_timeout(Duration::from_secs(8))?;
        let mut child = Command::new(env!("CARGO_BIN_EXE_madrillo"))
            .arg("--state-dir")
            .arg(&state)
            .args(["--log-level", "trace", "--timeout", "1", "--"])
            .arg(env!("CARGO_BIN_EXE_madrillo-fixture"))
            .arg("sleep")
            .stdout(Stdio::null())
            .stderr(Stdio::from(writer))
            .spawn()?;
        wait(&mut child)?;
        Ok(serde_json::from_slice(&fs::read(
            state.join("result.json"),
        )?)?)
    })();
    drop(reader);
    let fill_error = filler
        .join()
        .map_err(|_| "diagnostic pipe filler panicked")?
        .err()
        .ok_or("diagnostic pipe filler unexpectedly finished")?;
    assert_eq!(fill_error.kind(), std::io::ErrorKind::BrokenPipe);
    let result = outcome?;
    assert_eq!(result.cause, CompletionCause::TimedOut);
    assert!(
        result
            .monitor_errors
            .iter()
            .any(|error| error.contains("diagnostics flush deadline")),
        "{result:?}"
    );
    Ok(())
}
