use madrillo::{CleanupResult, LeaderResult, OomEvidence};
use std::{
    io,
    process::{ChildStderr, ChildStdout, Command, ExitStatus},
};

pub(crate) struct Workload {
    observed: Option<LeaderResult>,
    #[cfg(target_os = "linux")]
    group: Option<crate::cgroup::Cgroup>,
    #[cfg(unix)]
    child: std::process::Child,
    #[cfg(windows)]
    child: WindowsChild,
}

impl Workload {
    pub(crate) fn id(&self) -> u32 {
        #[cfg(unix)]
        {
            self.child.id()
        }
        #[cfg(windows)]
        {
            match &self.child {
                WindowsChild::Pipe(child) => child.id(),
                WindowsChild::Terminal(child) => child.id(),
            }
        }
    }
    pub(crate) fn spawn(
        command: Command,
        terminal: Option<&mut crate::terminal::Terminal>,
        #[cfg(target_os = "linux")] group: Option<crate::cgroup::Cgroup>,
        #[cfg(unix)] inherited: Vec<command_fds::FdMapping>,
    ) -> io::Result<Self> {
        #[cfg(unix)]
        {
            let mut command = command;
            if let Some(terminal) = terminal {
                terminal.configure(&mut command)?;
            }
            #[cfg(target_os = "linux")]
            nix::sys::prctl::set_child_subreaper(true)?;
            #[cfg(target_os = "linux")]
            let membership = group.as_ref().map(|group| group.membership()).transpose()?;
            #[cfg(not(target_os = "linux"))]
            let membership = None;
            let child = crate::launch::spawn(command, membership, inherited)?;
            Ok(Self {
                observed: None,
                child,
                #[cfg(target_os = "linux")]
                group,
            })
        }

        #[cfg(windows)]
        {
            let child = if let Some(terminal) = terminal {
                WindowsChild::Terminal(terminal.spawn(command)?)
            } else {
                use process_wrap::std::{CommandWrap, JobObject};
                WindowsChild::Pipe(CommandWrap::from(command).wrap(JobObject).spawn()?)
            };
            Ok(Self {
                child,
                observed: None,
            })
        }
    }
    pub(crate) fn stdout(&mut self) -> Option<ChildStdout> {
        #[cfg(unix)]
        {
            self.child.stdout.take()
        }
        #[cfg(windows)]
        {
            match &mut self.child {
                WindowsChild::Pipe(child) => child.stdout().take(),
                WindowsChild::Terminal(_) => None,
            }
        }
    }
    pub(crate) fn stderr(&mut self) -> Option<ChildStderr> {
        #[cfg(unix)]
        {
            self.child.stderr.take()
        }
        #[cfg(windows)]
        {
            match &mut self.child {
                WindowsChild::Pipe(child) => child.stderr().take(),
                WindowsChild::Terminal(_) => None,
            }
        }
    }
    pub(crate) fn exited(&mut self) -> io::Result<bool> {
        #[cfg(target_os = "linux")]
        {
            use nix::sys::wait::{Id, WaitPidFlag, waitid, waitpid};
            // Reap adopted orphans throughout the workload's lifetime. Keep
            // the leader waitable so its process-group ID cannot be recycled.
            for _ in 0..64 {
                let status = waitid(
                    Id::All,
                    WaitPidFlag::WEXITED | WaitPidFlag::WNOHANG | WaitPidFlag::WNOWAIT,
                )?;
                let Some(pid) = status.pid() else {
                    return Ok(false);
                };
                if u32::try_from(pid.as_raw()).map_err(io::Error::other)? == self.child.id() {
                    self.observed = match status {
                        nix::sys::wait::WaitStatus::Exited(_, code) => {
                            Some(LeaderResult::Exit { code: code as u32 })
                        }
                        nix::sys::wait::WaitStatus::Signaled(_, signal, core_dumped) => {
                            Some(LeaderResult::Signal {
                                signal: signal as u32,
                                core_dumped,
                            })
                        }
                        _ => None,
                    };
                    return Ok(self.observed.is_some());
                }
                waitpid(pid, Some(WaitPidFlag::WNOHANG))?;
            }
            Ok(false)
        }
        #[cfg(all(unix, not(target_os = "linux")))]
        {
            use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
            let pid = Pid::from_raw(i32::try_from(self.child.id()).map_err(io::Error::other)?)
                .ok_or_else(|| io::Error::other("invalid child PID"))?;
            // Keep the leader waitable until scope cleanup, preventing PID reuse.
            let status = waitid(
                WaitId::Pid(pid),
                WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
            )?;
            if let Some(status) = status {
                self.observed = status
                    .exit_status()
                    .map(|code| LeaderResult::Exit { code: code as u32 })
                    .or_else(|| {
                        status
                            .terminating_signal()
                            .map(|signal| LeaderResult::Signal {
                                signal: signal as u32,
                                core_dumped: status.dumped(),
                            })
                    });
                // A stopped or continued child is still a live workload.
                // Require terminal status even if the host returns another
                // kind of event despite the EXITED selection above.
                Ok(self.observed.is_some())
            } else {
                Ok(false)
            }
        }
        #[cfg(windows)]
        {
            self.observed =
                match &mut self.child {
                    // process-wrap's outer try_wait consumes Job completion events.
                    // Poll the leader directly; reserve the Job event for final wait.
                    WindowsChild::Pipe(child) => child.inner_mut().try_wait()?.map(native_result),
                    WindowsChild::Terminal(child) => child
                        .try_wait()
                        .map_err(io::Error::other)?
                        .map(|status| LeaderResult::Exit {
                            code: status.code(),
                        }),
                };
            Ok(self.observed.is_some())
        }
    }
    pub(crate) fn force(&mut self) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        if let Some(group) = &mut self.group {
            return group.force();
        }
        #[cfg(unix)]
        {
            use nix::{
                errno::Errno,
                sys::signal::{Signal, killpg},
                unistd::Pid,
            };
            let pid = i32::try_from(self.child.id()).map_err(io::Error::other)?;
            match killpg(Pid::from_raw(pid), Signal::SIGKILL) {
                Ok(()) | Err(Errno::ESRCH) => Ok(()),
                Err(error) => Err(error.into()),
            }
        }
        #[cfg(windows)]
        {
            match &mut self.child {
                WindowsChild::Pipe(child) => child.start_kill(),
                WindowsChild::Terminal(child) => child.kill().map_err(io::Error::other),
            }
        }
    }
    #[cfg(unix)]
    pub(crate) fn signal(&self, signal: i32) -> io::Result<()> {
        use nix::{
            errno::Errno,
            sys::signal::{Signal, killpg},
            unistd::Pid,
        };
        let pid = i32::try_from(self.child.id()).map_err(io::Error::other)?;
        match killpg(Pid::from_raw(pid), Signal::try_from(signal)?) {
            Ok(()) | Err(Errno::ESRCH) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
    fn oom(&self) -> OomEvidence {
        #[cfg(target_os = "linux")]
        {
            self.group
                .as_ref()
                .map(|group| group.oom())
                .unwrap_or_else(|| madrillo::OomEvidence::Unavailable {
                    reason: "no cgroup supplied".into(),
                })
        }
        #[cfg(not(target_os = "linux"))]
        {
            OomEvidence::Unavailable {
                reason: "native OOM observation unavailable on this host".into(),
            }
        }
    }
    pub(crate) fn finish(
        mut self,
        mut errors: Vec<String>,
    ) -> (LeaderResult, CleanupResult, OomEvidence) {
        use std::{sync::mpsc, thread, time::Duration};
        let known = self.observed.clone().unwrap_or(LeaderResult::Unknown);
        let (done, complete) = mpsc::sync_channel(1);
        let spawned = thread::Builder::new()
            .name("madrillo-reap".into())
            .spawn(move || {
                let leader = match self.reap() {
                    Ok(leader) => leader,
                    Err(error) => {
                        errors.push(format!("reap workload: {error}"));
                        self.observed.clone().unwrap_or(LeaderResult::Unknown)
                    }
                };
                let cleanup = self.cleanup_result(errors);
                let _ = done.send((leader, cleanup, self.oom()));
            });
        match spawned {
            Ok(_) => match complete.recv_timeout(Duration::from_secs(5)) {
                Ok(result) => result,
                Err(error) => (
                    known,
                    CleanupResult::Incomplete {
                        errors: vec![format!("workload reaping did not complete: {error}")],
                    },
                    OomEvidence::Unavailable {
                        reason: "final workload observation timed out".into(),
                    },
                ),
            },
            Err(error) => (
                known,
                CleanupResult::Incomplete {
                    errors: vec![format!("create reap worker: {error}")],
                },
                OomEvidence::Unavailable {
                    reason: "final workload observation unavailable".into(),
                },
            ),
        }
    }
    pub(crate) fn reap(&mut self) -> io::Result<LeaderResult> {
        #[cfg(unix)]
        {
            Ok(native_result(self.child.wait()?))
        }
        #[cfg(windows)]
        {
            match &mut self.child {
                WindowsChild::Pipe(child) => Ok(native_result(child.wait()?)),
                WindowsChild::Terminal(child) => Ok(LeaderResult::Exit {
                    code: child.wait().map_err(io::Error::other)?.code(),
                }),
            }
        }
    }
    fn cleanup_result(&self, errors: Vec<String>) -> CleanupResult {
        let mut errors = errors;
        #[cfg(windows)]
        if matches!(self.child, WindowsChild::Terminal(_)) {
            errors.push("ConPTY job termination was requested, but the dependency does not expose a job-empty observation".into());
        }
        #[cfg(target_os = "linux")]
        if let Some(group) = &self.group {
            if let Err(error) = group.wait_empty() {
                errors.push(error.to_string());
            }
        } else {
            errors.push("process-group cleanup cannot guarantee removal of escaped descendants; no cgroup supplied".into());
        }
        #[cfg(target_os = "linux")]
        if let Err(error) = reap_descendants() {
            errors.push(error.to_string());
        }
        #[cfg(all(unix, not(target_os = "linux")))]
        errors.push("process-group cleanup cannot guarantee removal of escaped descendants; native workload scope is not implemented yet".into());
        if errors.is_empty() {
            CleanupResult::Complete
        } else {
            CleanupResult::Incomplete { errors }
        }
    }
}

fn native_result(status: ExitStatus) -> LeaderResult {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return LeaderResult::Signal {
                signal: signal as u32,
                core_dumped: status.core_dumped(),
            };
        }
    }
    match status.code() {
        // Windows code() uses i32 to carry all 32 native bits.
        Some(code) => LeaderResult::Exit { code: code as u32 },
        None => LeaderResult::Unknown,
    }
}

#[cfg(target_os = "linux")]
fn reap_descendants() -> io::Result<()> {
    use nix::{
        errno::Errno,
        sys::wait::{WaitPidFlag, WaitStatus, waitpid},
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        match waitpid(None, Some(WaitPidFlag::WNOHANG)) {
            Err(Errno::ECHILD) => return Ok(()),
            Err(Errno::EINTR) => {}
            Err(error) => return Err(error.into()),
            Ok(WaitStatus::StillAlive) if std::time::Instant::now() >= deadline => {
                return Err(io::Error::other(
                    "adopted descendants remain live after scope cleanup",
                ));
            }
            Ok(WaitStatus::StillAlive) => std::thread::sleep(std::time::Duration::from_millis(2)),
            Ok(_) => {}
        }
    }
}

#[cfg(windows)]
enum WindowsChild {
    Pipe(Box<dyn process_wrap::std::ChildWrapper>),
    Terminal(conpty_oxide::blocking::Child),
}
