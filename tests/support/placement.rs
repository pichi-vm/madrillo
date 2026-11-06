use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub(crate) fn probe(root: &Path, terminal: bool) -> Result<i32> {
    #[cfg(unix)]
    {
        use nix::unistd::{getpgid, getpgrp, getpid, getppid, getsid, tcgetpgrp};
        let own_group = getpgrp();
        if own_group != getpid() || getpgid(Some(getppid()))? == own_group {
            return Err("workload group was not isolated before execution".into());
        }
        if terminal && (getsid(None)? != getpid() || tcgetpgrp(std::io::stdin())? != own_group) {
            return Err("controlling terminal/session was not established before execution".into());
        }
    }
    #[cfg(windows)]
    {
        let _ = terminal;
        if breakaway(root)? {
            return Err("first workload spawn escaped its Job".into());
        }
    }
    let leader = std::process::id();
    fs::write(root.join("leader"), leader.to_string())?;
    let mut helper = Command::new(std::env::current_exe()?);
    helper
        .arg("scope-helper")
        .arg(root)
        .arg(leader.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(fs::File::create(root.join("helper-error"))?));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // A new console process group and detached console cannot escape a Job.
        helper.creation_flags(0x0000_0200 | 0x0000_0008);
    }
    if !helper.status()?.success() {
        return Err(format!(
            "ordinary helper did not inherit workload placement: {}",
            fs::read_to_string(root.join("helper-error"))?
        )
        .into());
    }
    Ok(37)
}

pub(crate) fn helper(root: &Path, leader: u32) -> Result<i32> {
    #[cfg(unix)]
    if u32::try_from(nix::unistd::getpgrp().as_raw())? != leader {
        return Err("first helper code ran outside the workload group".into());
    }
    #[cfg(windows)]
    {
        let _ = leader;
        if breakaway(root)? {
            return Err("first helper spawn escaped its inherited Job".into());
        }
    }
    fs::write(root.join("helper"), b"scoped")?;
    Ok(0)
}

pub(crate) fn outside(root: &Path) -> Result<i32> {
    #[cfg(unix)]
    {
        let leader = fs::read_to_string(root.join("leader"))?.parse::<i32>()?;
        if nix::unistd::getpgrp().as_raw() == leader {
            return Err("exit helper inherited the workload group".into());
        }
    }
    #[cfg(windows)]
    if !breakaway(root)? {
        return Err("outside control/exit helper inherited workload restrictions".into());
    }
    fs::write(root.join("outside"), b"outside")?;
    Ok(0)
}

#[cfg(windows)]
fn breakaway(root: &Path) -> Result<bool> {
    use std::os::windows::process::CommandExt;
    let marker = root.join("breakaway");
    let result = Command::new(std::env::current_exe()?)
        .arg("touch")
        .arg(&marker)
        .creation_flags(0x0100_0000 | 0x0000_0008)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match result {
        Ok(status) if status.success() => {
            if fs::read(&marker)? != b"invoked" {
                return Err("breakaway control did not execute".into());
            }
            fs::remove_file(marker)?;
            Ok(true)
        }
        Err(error) if error.raw_os_error() == Some(5) => Ok(false),
        other => Err(format!("unexpected breakaway result: {other:?}").into()),
    }
}
