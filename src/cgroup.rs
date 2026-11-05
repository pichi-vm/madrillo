//! Caller-owned cgroup v2 containment. Madrillo never changes ancestor limits.
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use madrillo::OomEvidence;
use std::{
    fs::File,
    io::{self, Read, Write},
    path::Path,
    time::{Duration, Instant},
};

pub(crate) struct Cgroup {
    directory: Dir,
    kill: File,
    baseline: io::Result<(u64, u64)>,
}
impl Cgroup {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let directory = Dir::open_ambient_dir(path, ambient_authority())?;
        if nix::sys::statfs::fstatfs(&directory)?.filesystem_type()
            != nix::sys::statfs::CGROUP2_SUPER_MAGIC
        {
            return Err(io::Error::other(
                "--cgroup must refer to a real cgroup v2 directory",
            ));
        }
        if counter(&read(&directory, "cgroup.events")?, "populated")? != 0 {
            return Err(io::Error::other("--cgroup must be empty before launch"));
        }
        let kill = writable(&directory, "cgroup.kill")?;
        let baseline = memory(&directory);
        Ok(Self {
            directory,
            kill,
            baseline,
        })
    }
    pub(crate) fn membership(&self) -> io::Result<File> {
        writable(&self.directory, "cgroup.procs")
    }
    pub(crate) fn force(&mut self) -> io::Result<()> {
        self.kill.write_all(b"1")
    }
    pub(crate) fn wait_empty(&self) -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if counter(&read(&self.directory, "cgroup.events")?, "populated")? == 0 {
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
    pub(crate) fn oom(&self) -> OomEvidence {
        let delta = (|| -> io::Result<(u64, u64)> {
            let (before_events, before_kills) = self
                .baseline
                .as_ref()
                .map_err(|e| io::Error::other(e.to_string()))?;
            let (events, kills) = memory(&self.directory)?;
            Ok((
                events
                    .checked_sub(*before_events)
                    .ok_or_else(|| io::Error::other("OOM event counter decreased"))?,
                kills
                    .checked_sub(*before_kills)
                    .ok_or_else(|| io::Error::other("OOM kill counter decreased"))?,
            ))
        })();
        match delta {
            Ok((0, 0)) => OomEvidence::NoneObserved,
            Ok((events, kills)) => OomEvidence::Observed { events, kills },
            Err(error) => OomEvidence::Unavailable {
                reason: error.to_string(),
            },
        }
    }
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
