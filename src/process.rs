use madrillo::{CleanupResult, LeaderResult};
use std::{
    io,
    process::{ChildStderr, ChildStdout, Command, ExitStatus},
};

pub(crate) struct Workload {
    #[cfg(unix)]
    child: std::process::Child,
    #[cfg(windows)]
    child: Box<dyn process_wrap::std::ChildWrapper>,
}

impl Workload {
    pub(crate) fn id(&self) -> u32 {
        self.child.id()
    }
    pub(crate) fn spawn(
        command: Command,
        terminal: Option<&mut crate::terminal::Terminal>,
    ) -> io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            if let Some(terminal) = terminal {
                return Ok(Self {
                    child: terminal.spawn(command)?,
                });
            }
            let mut command = command;
            command.process_group(0);
            Ok(Self {
                child: command.spawn()?,
            })
        }
        #[cfg(windows)]
        {
            if terminal.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "ConPTY is not implemented yet",
                ));
            }
            use process_wrap::std::{CommandWrap, JobObject};
            let child = CommandWrap::from(command).wrap(JobObject).spawn()?;
            Ok(Self { child })
        }
    }
    pub(crate) fn stdout(&mut self) -> Option<ChildStdout> {
        #[cfg(unix)]
        {
            self.child.stdout.take()
        }
        #[cfg(windows)]
        {
            self.child.stdout().take()
        }
    }
    pub(crate) fn stderr(&mut self) -> Option<ChildStderr> {
        #[cfg(unix)]
        {
            self.child.stderr.take()
        }
        #[cfg(windows)]
        {
            self.child.stderr().take()
        }
    }
    pub(crate) fn exited(&mut self) -> io::Result<bool> {
        #[cfg(unix)]
        {
            use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
            let pid = Pid::from_raw(i32::try_from(self.child.id()).map_err(io::Error::other)?)
                .ok_or_else(|| io::Error::other("invalid child PID"))?;
            // Keep the leader waitable until scope cleanup, preventing PID reuse.
            Ok(waitid(
                WaitId::Pid(pid),
                WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
            )?
            .is_some())
        }
        #[cfg(windows)]
        {
            Ok(self.child.try_wait()?.is_some())
        }
    }
    pub(crate) fn force(&mut self) -> io::Result<()> {
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
            self.child.start_kill()
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
    pub(crate) fn finish(mut self, mut errors: Vec<String>) -> (LeaderResult, CleanupResult) {
        use std::{sync::mpsc, thread, time::Duration};
        let (done, complete) = mpsc::sync_channel(1);
        let spawned = thread::Builder::new()
            .name("madrillo-reap".into())
            .spawn(move || {
                let leader = match self.reap() {
                    Ok(leader) => leader,
                    Err(error) => {
                        errors.push(format!("reap workload: {error}"));
                        LeaderResult::Unknown
                    }
                };
                let _ = done.send((leader, self.cleanup_result(errors)));
            });
        match spawned {
            Ok(_) => match complete.recv_timeout(Duration::from_secs(5)) {
                Ok(result) => result,
                Err(error) => (
                    LeaderResult::Unknown,
                    CleanupResult::Incomplete {
                        errors: vec![format!("workload reaping did not complete: {error}")],
                    },
                ),
            },
            Err(error) => (
                LeaderResult::Unknown,
                CleanupResult::Incomplete {
                    errors: vec![format!("create reap worker: {error}")],
                },
            ),
        }
    }
    pub(crate) fn reap(&mut self) -> io::Result<LeaderResult> {
        Ok(native_result(self.child.wait()?))
    }
    pub(crate) fn cleanup_result(&self, errors: Vec<String>) -> CleanupResult {
        #[cfg(unix)]
        let errors = {
            let mut errors = errors;
            errors.push("process-group cleanup cannot guarantee removal of escaped descendants; native workload scope is not implemented yet".to_owned());
            errors
        };
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
