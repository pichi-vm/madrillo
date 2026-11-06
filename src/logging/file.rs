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
    path::{Path, PathBuf},
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
    pub(crate) fn open(
        path: &Path,
        limits: Limits,
        synchronize: bool,
        allowlist: &[PathBuf],
    ) -> io::Result<Self> {
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::other("log path needs a filename"))?
            .to_owned();
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let directory = Self::directory(parent, allowlist)?;
        #[cfg(unix)]
        {
            use cap_std::fs::MetadataExt;
            let metadata = directory.dir_metadata()?;
            if metadata.mode() & 0o002 != 0
                || ![
                    0,
                    rustix::process::getuid().as_raw(),
                    rustix::process::geteuid().as_raw(),
                ]
                .contains(&metadata.uid())
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "log directory has untrusted ownership or is world-writable",
                ));
            }
        }
        #[cfg(windows)]
        {
            use win_custody::{ObjectSecurityDescriptor, OwnedSid};
            let descriptor = ObjectSecurityDescriptor::query_kernel_object(&directory)?;
            let user = OwnedSid::from_string(&win_custody::current_user_sid_string()?)?;
            let system = OwnedSid::from_string("S-1-5-18")?;
            let administrators = OwnedSid::from_string("S-1-5-32-544")?;
            let creator = OwnedSid::from_string("S-1-3-0")?;
            let trusted = [&user, &system, &administrators];
            let owner = descriptor.owner().map_err(io::Error::other)?;
            let acl = descriptor.dacl().map_err(io::Error::other)?;
            // GENERIC_READ | GENERIC_EXECUTE | FILE_GENERIC_READ |
            // FILE_GENERIC_EXECUTE. Other accounts may read logs, but cannot
            // replace entries, modify inherited files or change permissions.
            const READ_ONLY: u32 = 0xa012_00a9;
            if !trusted.iter().any(|sid| owner.matches(sid))
                || !acl.allowed_aces().all(|ace| {
                    ace.mask() & !READ_ONLY == 0
                        || trusted.iter().any(|sid| ace.trustee_matches(sid))
                        || (ace.trustee_matches(&creator) && ace.flags() & 0x08 != 0)
                })
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "log directory has untrusted ownership or grants modification to another account",
                ));
            }
        }
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

    fn directory(parent: &Path, allowlist: &[PathBuf]) -> io::Result<Dir> {
        if allowlist.is_empty() {
            return Dir::open_ambient_dir(parent, ambient_authority());
        }
        if allowlist.len() > 64 {
            return Err(io::Error::other(
                "at most 64 log allowlist directories are supported",
            ));
        }
        let parent = std::fs::canonicalize(parent)?;
        for allowed in allowlist {
            if allowed.as_os_str().is_empty() {
                continue;
            }
            let Ok(root) = Dir::open_ambient_dir(allowed, ambient_authority()) else {
                continue;
            };
            let Ok(canonical) = std::fs::canonicalize(allowed) else {
                continue;
            };
            let Ok(relative) = parent.strip_prefix(canonical) else {
                continue;
            };
            // Resolution is rooted in the granted directory capability. A
            // rename/symlink race cannot redirect this open outside that grant.
            return root.open_dir(if relative.as_os_str().is_empty() {
                Path::new(".")
            } else {
                relative
            });
        }
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "log directory is outside the allowlist",
        ))
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
            self.write_record(&record)?;
        }
        Ok(())
    }

    /// Opaque streams use raw bytes, never invented CRI stream names.
    pub(crate) fn write_raw(&mut self, mut bytes: &[u8]) -> io::Result<()> {
        if self.limits.size == Some(0) {
            return Err(io::Error::other("raw log size limit must be nonzero"));
        }
        while !bytes.is_empty() {
            if self.limits.total.is_some_and(|limit| self.total >= limit) {
                break;
            }
            if self.limits.size.is_some_and(|limit| self.size >= limit) {
                self.reopen()?;
            }
            let mut keep = bytes.len();
            for remaining in [
                self.limits
                    .size
                    .map(|limit| limit.saturating_sub(self.size)),
                self.limits
                    .total
                    .map(|limit| limit.saturating_sub(self.total)),
            ]
            .into_iter()
            .flatten()
            {
                keep = keep.min(usize::try_from(remaining).unwrap_or(usize::MAX));
            }
            self.write_record(&bytes[..keep])?;
            bytes = &bytes[keep..];
        }
        Ok(())
    }

    fn write_record(&mut self, bytes: &[u8]) -> io::Result<()> {
        let length = u64::try_from(bytes.len()).map_err(io::Error::other)?;
        if self
            .limits
            .size
            .is_some_and(|limit| self.size.saturating_add(length) > limit)
        {
            self.reopen()?;
        }
        self.file.write_all(bytes)?;
        self.size = self.size.saturating_add(length);
        self.total = self.total.saturating_add(length);
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
