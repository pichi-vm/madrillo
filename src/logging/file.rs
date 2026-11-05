use super::cri::{Cri, Stream};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, File, OpenOptions},
};
use chrono::{DateTime, FixedOffset};
use std::{
    ffi::OsString,
    io::{self, Write},
    path::Path,
};

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Limits {
    pub(crate) size: Option<u64>,
    pub(crate) total: Option<u64>,
    pub(crate) backups: Option<u16>,
}

pub(crate) struct FileLog {
    directory: Dir,
    name: OsString,
    file: File,
    limits: Limits,
    size: u64,
    total: u64,
    synchronize: bool,
}

impl FileLog {
    pub(crate) fn open(path: &Path, limits: Limits, synchronize: bool) -> io::Result<Self> {
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::other("log path needs a filename"))?
            .to_owned();
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        // The selected directory is the caller's grant. All subsequent opens
        // and renames stay relative to this held directory, even if it moves.
        let directory = Dir::open_ambient_dir(parent, ambient_authority())?;
        let file = Self::open_file(&directory, &name, false)?;
        let size = file.metadata()?.len();
        Ok(Self {
            directory,
            name,
            file,
            limits,
            size,
            total: size,
            synchronize,
        })
    }

    fn open_file(directory: &Dir, name: &OsString, new: bool) -> io::Result<File> {
        let mut options = OpenOptions::new();
        options.append(true).follow(FollowSymlinks::No);
        if new {
            options.create_new(true);
        } else {
            options.create(true);
        }
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o640);
            options.custom_flags(nix::libc::O_NONBLOCK);
        }
        let file = directory.open_with(name, &options)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::other("log destination must be a regular file"));
        }
        #[cfg(unix)]
        {
            use cap_std::fs::MetadataExt;
            if file.metadata()?.nlink() != 1 {
                return Err(io::Error::other(
                    "log destination must have exactly one hard link",
                ));
            }
        }
        #[cfg(windows)]
        {
            use cap_fs_ext::MetadataExt;
            if file.metadata()?.nlink() != 1 {
                return Err(io::Error::other(
                    "log destination must have exactly one hard link",
                ));
            }
        }
        Ok(file)
    }

    pub(crate) fn write_chunk(
        &mut self,
        time: DateTime<FixedOffset>,
        stream: Stream,
        bytes: &[u8],
    ) -> io::Result<()> {
        for line in bytes.split_inclusive(|b| *b == b'\n') {
            if self.limits.total.is_some_and(|limit| self.total >= limit) {
                break;
            }
            let mut record = Vec::new();
            record
                .try_reserve(line.len().saturating_add(64))
                .map_err(|_| io::Error::other("log record allocation failed"))?;
            Cri::new(&mut record).write_chunk(time, stream, line)?;
            let length = u64::try_from(record.len()).map_err(io::Error::other)?;
            if self
                .limits
                .size
                .is_some_and(|limit| self.size.saturating_add(length) > limit)
            {
                self.reopen()?;
            }
            self.file.write_all(&record)?;
            self.size = self.size.saturating_add(length);
            self.total = self.total.saturating_add(length);
        }
        Ok(())
    }

    fn backup(&self, index: u16) -> OsString {
        let mut name = self.name.clone();
        name.push(format!(".{index}"));
        name
    }

    // Conmon's reopen means rotate/truncate, including an explicit reopen
    // request; it is not merely an append-open of a replacement inode.
    pub(crate) fn reopen(&mut self) -> io::Result<()> {
        // Prepare the replacement before touching the active log. The held
        // directory capability keeps every operation in the granted directory.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let (name, file) = (0..32)
            .find_map(|_| {
                let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let name =
                    OsString::from(format!(".madrillo-{}-{sequence}.log", std::process::id()));
                match Self::open_file(&self.directory, &name, true) {
                    Ok(file) => Some(Ok((name, file))),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => None,
                    Err(error) => Some(Err(error)),
                }
            })
            .ok_or_else(|| io::Error::other("temporary log name collisions"))??;
        let replace = (|| -> io::Result<()> {
            if self.synchronize {
                self.file.sync_all()?;
            }
            if let Some(backups) = self.limits.backups {
                for index in (2..=backups).rev() {
                    match self.directory.rename(
                        self.backup(index - 1),
                        &self.directory,
                        self.backup(index),
                    ) {
                        Ok(()) => {}
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error),
                    }
                }
                self.directory
                    .rename(&self.name, &self.directory, self.backup(1))?;
                if let Err(error) = self.directory.rename(&name, &self.directory, &self.name) {
                    if let Err(restore) =
                        self.directory
                            .rename(self.backup(1), &self.directory, &self.name)
                    {
                        return Err(io::Error::other(format!(
                            "replace log: {error}; restore original: {restore}"
                        )));
                    }
                    return Err(error);
                }
            } else {
                // Atomic directory-entry replacement does not follow a link
                // placed at the destination and never truncates its target.
                self.directory.rename(&name, &self.directory, &self.name)?;
            }
            Ok(())
        })();
        if let Err(error) = replace {
            let _ = self.directory.remove_file(name);
            return Err(error);
        }
        self.file = file;
        self.size = 0;
        Ok(())
    }

    pub(crate) fn sync(&self) -> io::Result<()> {
        self.file.sync_all()?;
        #[cfg(unix)]
        self.directory.open(".")?.sync_all()?;
        Ok(())
    }
}
