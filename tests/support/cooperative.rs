//! An ordinary pipe workload owns its helper's cooperative shutdown.
//! Console writes detect monitor loss even while stdin is deliberately unread.
use std::{
    fs,
    io::{self, Write},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub(crate) fn leader(root: &Path) -> Result<i32> {
    let mut helper = Command::new(std::env::current_exe()?)
        .arg("cooperative-helper")
        .arg(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let result = (|| -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match io::stdout().write_all(b"alive\n") {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::BrokenPipe => break,
                Err(error) => return Err(error.into()),
            }
            if Instant::now() >= deadline {
                return Err("monitor console stayed open".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    })();
    // Only the workload owns this pipe. A clean helper exit proves that closing
    // the monitor's Job/handles did not kill the workload tree first.
    drop(helper.stdin.take());
    let status = helper.wait()?;
    result?;
    if !status.success() || fs::read(root.join("helper-clean"))? != b"clean" {
        return Err("helper did not shut down cooperatively".into());
    }
    fs::write(root.join("leader-clean"), b"clean")?;
    Ok(0)
}

pub(crate) fn helper(root: &Path) -> Result<i32> {
    io::copy(&mut io::stdin(), &mut io::sink())?;
    fs::write(root.join("helper-clean"), b"clean")?;
    Ok(0)
}
