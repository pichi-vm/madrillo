use madrillo::{CleanupResult, LeaderResult, OomEvidence};
use std::{
    io,
    process::{ChildStderr, ChildStdout, Command, ExitStatus},
};

pub(crate) struct Workload {
    observed: Option<LeaderResult>,
    exited_at_unix_ms: u64,
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
                exited_at_unix_ms: 0,
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
                exited_at_unix_ms: 0,
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
        let exited = self.observe_exit()?;
        if exited && self.exited_at_unix_ms == 0 {
            self.exited_at_unix_ms = crate::unix_ms();
        }
        Ok(exited)
    }
    fn observe_exit(&mut self) -> io::Result<bool> {
        #[cfg(target_os = "linux")]
        {
            use nix::sys::wait::{Id, WaitPidFlag, waitid, waitpid};
            // Reap adopted orphans throughout the workload's lifetime. Keep
            // the leader waitable so its process-group ID cannot be recycled.
            for _ in 0..64 {
                let status = waitid(
                    Id::All,
                    WaitPidFlag::WEXITED
                        | WaitPidFlag::WNOHANG
                        | WaitPidFlag::WNOWAIT
                        | WaitPidFlag::__WALL,
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
                waitpid(pid, Some(WaitPidFlag::WNOHANG | WaitPidFlag::__WALL))?;
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
        {
            let scope = self.group.as_mut().map(|group| group.force()).transpose();
            // Always terminate the owned leader, even if it changed groups
            // or left the supplied cgroup. Subreaping then adopts its children.
            let leader = self.child.kill();
            scope?;
            leader
        }
        #[cfg(target_os = "macos")]
        {
            force_group_macos(self.child.id())
        }
        #[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
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
    #[cfg(target_os = "linux")]
    pub(crate) fn poll_oom(&mut self) -> io::Result<bool> {
        self.group
            .as_mut()
            .map_or(Ok(false), |group| group.poll_oom())
    }
    fn oom(&mut self) -> OomEvidence {
        #[cfg(target_os = "linux")]
        {
            self.group
                .as_mut()
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
    ) -> (LeaderResult, CleanupResult, OomEvidence, u64) {
        use std::{sync::mpsc, thread, time::Duration};
        let known = self.observed.clone().unwrap_or(LeaderResult::Unknown);
        let known_exit_time = self.exited_at_unix_ms;
        let (observed, observation) = mpsc::sync_channel(1);
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
                // Preserve leader status even when descendant cleanup stalls.
                let _ = observed.send((leader.clone(), self.exited_at_unix_ms));
                let cleanup = self.cleanup_result(errors);
                let _ = done.send((leader, cleanup, self.oom(), self.exited_at_unix_ms));
            });
        match spawned {
            Ok(_) => match complete.recv_timeout(Duration::from_secs(5)) {
                Ok(result) => result,
                Err(error) => {
                    let (known, known_exit_time) =
                        observation.try_recv().unwrap_or((known, known_exit_time));
                    (
                        known,
                        CleanupResult::Incomplete {
                            errors: vec![format!("workload reaping did not complete: {error}")],
                        },
                        OomEvidence::Unavailable {
                            reason: "final workload observation timed out".into(),
                        },
                        known_exit_time,
                    )
                }
            },
            Err(error) => (
                known,
                CleanupResult::Incomplete {
                    errors: vec![format!("create reap worker: {error}")],
                },
                OomEvidence::Unavailable {
                    reason: "final workload observation unavailable".into(),
                },
                known_exit_time,
            ),
        }
    }
    fn reap(&mut self) -> io::Result<LeaderResult> {
        #[cfg(unix)]
        let leader = native_result(self.child.wait()?);
        #[cfg(windows)]
        let leader = match &mut self.child {
            // Observe the leader before the outer wrapper waits for the Job.
            WindowsChild::Pipe(child) => native_result(child.inner_mut().wait()?),
            WindowsChild::Terminal(child) => LeaderResult::Exit {
                code: child.wait().map_err(io::Error::other)?.code(),
            },
        };
        if self.exited_at_unix_ms == 0 {
            self.exited_at_unix_ms = crate::unix_ms();
        }
        self.observed = Some(leader.clone());
        Ok(leader)
    }
    fn cleanup_result(&mut self, errors: Vec<String>) -> CleanupResult {
        let mut errors = errors;
        #[cfg(windows)]
        if let WindowsChild::Pipe(child) = &mut self.child
            && let Err(error) = child.wait()
        {
            errors.push(format!("wait for workload Job: {error}"));
        }
        #[cfg(windows)]
        if matches!(self.child, WindowsChild::Terminal(_)) {
            errors.push("ConPTY job termination was requested, but the dependency does not expose a job-empty observation".into());
        }
        #[cfg(target_os = "linux")]
        if let Err(error) = reap_descendants() {
            errors.push(error.to_string());
        }
        #[cfg(target_os = "linux")]
        if let Some(group) = &self.group
            && let Err(error) = group.wait_empty()
        {
            errors.push(error.to_string());
        }
        #[cfg(target_os = "macos")]
        if let Err(error) = wait_group_empty(self.child.id()) {
            errors.push(error.to_string());
        }
        #[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
        errors.push("native workload cleanup is not qualified on this host".into());
        if errors.is_empty() {
            CleanupResult::Complete
        } else {
            CleanupResult::Incomplete { errors }
        }
    }
}

#[cfg(target_os = "macos")]
fn force_group_macos(leader: u32) -> io::Result<()> {
    use nix::{
        errno::Errno,
        sys::signal::{Signal, killpg},
        unistd::Pid,
    };
    use std::time::{Duration, Instant};
    let group = Pid::from_raw(i32::try_from(leader).map_err(io::Error::other)?);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        // XNU signals a membership snapshot. Repeat while the unreaped leader
        // reserves this group ID, covering children forked during delivery.
        match killpg(group, Signal::SIGKILL) {
            Err(Errno::ESRCH | Errno::EPERM) => return Ok(()),
            Err(error) if error != Errno::EINTR => return Err(error.into()),
            _ => {}
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(
                "workload process group remains signalable after termination",
            ));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(target_os = "macos")]
fn wait_group_empty(leader: u32) -> io::Result<()> {
    use nix::{errno::Errno, sys::signal::killpg, unistd::Pid};
    use std::time::{Duration, Instant};
    let group = Pid::from_raw(i32::try_from(leader).map_err(io::Error::other)?);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        // Force was issued while the leader was still waitable. After reaping
        // it, only observe the group; never signal a potentially reused ID.
        // The macOS contract covers members of this group, not escaped jobs.
        match killpg(group, None) {
            Err(Errno::ESRCH) => return Ok(()),
            // XNU can report EPERM for an existing group containing only
            // zombies. Keep observing until the group disappears; neither
            // permission denial nor a zombie-only group proves completion.
            Err(Errno::EINTR | Errno::EPERM) => {}
            Err(error) => return Err(error.into()),
            Ok(()) => {}
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(
                "workload process group remains after termination",
            ));
        }
        std::thread::sleep(Duration::from_millis(5));
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
const DESCENDANT_WAIT: rustix::process::WaitIdOptions =
    rustix::process::WaitIdOptions::from_bits_retain(
        rustix::process::WaitIdOptions::EXITED.bits()
            | rustix::process::WaitIdOptions::NOHANG.bits()
            | nix::sys::wait::WaitPidFlag::__WALL.bits() as u32,
    );

#[cfg(target_os = "linux")]
fn reap_descendants() -> io::Result<()> {
    use rustix::{
        io::Errno,
        process::{WaitId, waitid},
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        for _ in 0..64 {
            match waitid(WaitId::All, DESCENDANT_WAIT) {
                Err(Errno::CHILD) => return Ok(()),
                Err(Errno::INTR) => {}
                Err(error) => return Err(error.into()),
                Ok(None) => break,
                Ok(Some(_)) => {}
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(io::Error::other(
                "adopted descendants remain after termination",
            ));
        }
        terminate_adopted_children()?;
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

#[cfg(target_os = "linux")]
fn terminate_adopted_children() -> io::Result<()> {
    use rustix::{
        io::Errno,
        process::{Pid, PidfdFlags, Signal, WaitId, pidfd_open, pidfd_send_signal, waitid},
    };
    use std::{
        fs::File,
        io::{BufRead, BufReader},
        os::fd::AsFd,
    };
    // Madrillo establishes subreaping before launch and has no other child
    // workloads. An exiting ancestor reparents its remaining children here,
    // irrespective of their process groups, sessions or nested subreapers.
    // /proc enumeration can miss a concurrent adoption; ECHILD from waitid,
    // never an empty enumeration, is the final completion test.
    for task in std::fs::read_dir("/proc/self/task")? {
        let children = match File::open(task?.path().join("children")) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for token in BufReader::new(children).split(b' ') {
            let token = token?;
            if token.is_empty() {
                continue;
            }
            let number = std::str::from_utf8(&token)
                .map_err(io::Error::other)?
                .parse::<i32>()
                .map_err(io::Error::other)?;
            let pid =
                Pid::from_raw(number).ok_or_else(|| io::Error::other("invalid adopted PID"))?;
            let descriptor = match pidfd_open(pid, PidfdFlags::empty()) {
                Ok(descriptor) => descriptor,
                Err(Errno::SRCH) => continue,
                Err(error) => return Err(error.into()),
            };
            // Validate ownership through the same kernel identity we signal.
            // A reused numeric PID referring to a non-child yields ECHILD.
            match waitid(WaitId::PidFd(descriptor.as_fd()), DESCENDANT_WAIT) {
                Ok(Some(_)) | Err(Errno::CHILD | Errno::SRCH | Errno::INTR) => continue,
                Err(error) => return Err(error.into()),
                Ok(None) => {}
            }
            match pidfd_send_signal(descriptor, Signal::KILL) {
                Ok(()) | Err(Errno::SRCH) => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
enum WindowsChild {
    Pipe(Box<dyn process_wrap::std::ChildWrapper>),
    Terminal(conpty_oxide::blocking::Child),
}
