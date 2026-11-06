//! Caller-owned Linux cgroups. Madrillo never changes ancestor limits.
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use madrillo::OomEvidence;
use nix::sys::eventfd::{EfdFlags, EventFd};
use std::{
    fs::File,
    io::{self, Read, Write},
    os::fd::AsRawFd,
    path::Path,
    time::{Duration, Instant},
};

pub(crate) struct Cgroup {
    directory: Dir,
    kill: Option<File>,
    observation: io::Result<Observation>,
}

enum Observation {
    Unified {
        baseline: (u64, u64),
    },
    Legacy {
        _control: File,
        events: EventFd,
        baseline_kills: u64,
    },
}
impl Cgroup {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let directory = Dir::open_ambient_dir(path, ambient_authority())?;
        use nix::sys::statfs::{CGROUP_SUPER_MAGIC, CGROUP2_SUPER_MAGIC, fstatfs};
        let (kill, observation) = match fstatfs(&directory)?.filesystem_type() {
            CGROUP2_SUPER_MAGIC => (
                Some(writable(&directory, "cgroup.kill")?),
                memory(&directory).map(|baseline| Observation::Unified { baseline }),
            ),
            CGROUP_SUPER_MAGIC => (None, Observation::legacy(&directory)),
            _ => {
                return Err(io::Error::other(
                    "--cgroup must refer to a real cgroup directory",
                ));
            }
        };
        let group = Self {
            directory,
            kill,
            observation,
        };
        if !group.empty()? {
            return Err(io::Error::other("--cgroup must be empty before launch"));
        }
        Ok(group)
    }
    pub(crate) fn membership(&self) -> io::Result<File> {
        writable(&self.directory, "cgroup.procs")
    }
    pub(crate) fn force(&mut self) -> io::Result<()> {
        if let Some(kill) = &mut self.kill {
            kill.write_all(b"1")?;
        }
        // V1 has no atomic subtree kill. Leader termination followed by
        // subreaper/pidfd cleanup performs termination on this hierarchy.
        Ok(())
    }
    pub(crate) fn wait_empty(&self) -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if self.empty()? {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other(
                    "cgroup remains populated after termination",
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn empty(&self) -> io::Result<bool> {
        if self.kill.is_some() {
            return Ok(counter(&read(&self.directory, "cgroup.events")?, "populated")? == 0);
        }
        let mut pending = vec![self.directory.try_clone()?];
        let mut visited = 0;
        while let Some(directory) = pending.pop() {
            visited += 1;
            if visited > 1024 {
                return Err(io::Error::other("cgroup subtree exceeds observation limit"));
            }
            let mut byte = [0];
            if readable(&directory, "cgroup.procs")?.read(&mut byte)? != 0 {
                return Ok(false);
            }
            for entry in directory.entries()? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    if pending.len() >= 256 {
                        return Err(io::Error::other("cgroup subtree exceeds observation limit"));
                    }
                    pending.push(entry.open_dir()?);
                }
            }
        }
        Ok(true)
    }
    pub(crate) fn oom(&self) -> OomEvidence {
        let delta = self
            .observation
            .as_ref()
            .map_err(|error| io::Error::other(error.to_string()))
            .and_then(|observation| observation.delta(&self.directory));
        match delta {
            Ok((0, 0)) => OomEvidence::NoneObserved,
            Ok((events, kills)) => OomEvidence::Observed { events, kills },
            Err(error) => OomEvidence::Unavailable {
                reason: error.to_string(),
            },
        }
    }
}
impl Observation {
    fn legacy(directory: &Dir) -> io::Result<Self> {
        let baseline_kills = counter(&read(directory, "memory.oom_control")?, "oom_kill")?;
        let control = readable(directory, "memory.oom_control")?;
        let events = EventFd::from_flags(EfdFlags::EFD_CLOEXEC | EfdFlags::EFD_NONBLOCK)?;
        // Register before launching any workload code. The native eventfd
        // retains notifications even when the supervisor is busy draining I/O.
        let registration = format!("{} {}", events.as_raw_fd(), control.as_raw_fd());
        writable(directory, "cgroup.event_control")?.write_all(registration.as_bytes())?;
        Ok(Self::Legacy {
            _control: control,
            events,
            baseline_kills,
        })
    }
    fn delta(&self, directory: &Dir) -> io::Result<(u64, u64)> {
        let (events, kills, before_events, before_kills) = match self {
            Self::Unified {
                baseline: (before_events, before_kills),
            } => {
                let (events, kills) = memory(directory)?;
                (events, kills, *before_events, *before_kills)
            }
            Self::Legacy {
                events,
                baseline_kills,
                ..
            } => {
                // Read the still-present kernel counter first. Removal also
                // signals the eventfd and must never be called an OOM event.
                let kills = counter(&read(directory, "memory.oom_control")?, "oom_kill")?;
                let count = match events.read() {
                    Ok(count) => count,
                    Err(nix::errno::Errno::EAGAIN) => 0,
                    Err(error) => return Err(error.into()),
                };
                // Recheck after draining to reject concurrent scope removal.
                readable(directory, "memory.oom_control")?;
                (count, kills, 0, *baseline_kills)
            }
        };
        Ok((
            events
                .checked_sub(before_events)
                .ok_or_else(|| io::Error::other("OOM event counter decreased"))?,
            kills
                .checked_sub(before_kills)
                .ok_or_else(|| io::Error::other("OOM kill counter decreased"))?,
        ))
    }
}
fn readable(directory: &Dir, name: &str) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    Ok(directory.open_with(name, &options)?.into_std())
}
fn writable(directory: &Dir, name: &str) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).follow(FollowSymlinks::No);
    Ok(directory.open_with(name, &options)?.into_std())
}
fn read(directory: &Dir, name: &str) -> io::Result<String> {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let mut text = String::new();
    directory
        .open_with(name, &options)?
        .take(8193)
        .read_to_string(&mut text)?;
    if text.len() > 8192 {
        return Err(io::Error::other("cgroup control file exceeds limit"));
    }
    Ok(text)
}
fn counter(text: &str, key: &str) -> io::Result<u64> {
    text.lines()
        .filter_map(|line| line.split_once(' '))
        .find(|(name, _)| *name == key)
        .ok_or_else(|| io::Error::other(format!("cgroup counter {key} absent")))?
        .1
        .parse()
        .map_err(io::Error::other)
}
fn memory(directory: &Dir) -> io::Result<(u64, u64)> {
    let text = read(directory, "memory.events")?;
    Ok((counter(&text, "oom")?, counter(&text, "oom_kill")?))
}
