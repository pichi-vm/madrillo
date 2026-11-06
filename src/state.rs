use madrillo::Completion;
#[cfg(unix)]
use std::fs;
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

pub(crate) struct State {
    directory: PathBuf,
    oom: Option<std::sync::mpsc::Receiver<io::Result<()>>>,
}

impl State {
    pub(crate) fn create(path: &Path) -> io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            // Refuse reuse: a result must never belong to a different run.
            builder.create(path)?;
        }
        #[cfg(windows)]
        {
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let name = path
                .file_name()
                .ok_or_else(|| io::Error::other("state directory needs a name"))?;
            // The held parent grants private inheritance before creation. There
            // is no window with public access followed by an ACL repair.
            crate::security::state_parent(parent)?.create_dir(name)?;
        }
        Ok(Self {
            directory: path.to_owned(),
            oom: None,
        })
    }

    pub(crate) fn retain_oom(&mut self) -> io::Result<()> {
        if self.oom.is_some() {
            return Ok(());
        }
        let directory = self.directory.clone();
        let (done, complete) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("madrillo-oom-state".into())
            .spawn(move || {
                let write = || -> io::Result<()> {
                    std::fs::File::options()
                        .write(true)
                        .create_new(true)
                        .open(directory.join("oom"))?
                        .sync_all()?;
                    #[cfg(unix)]
                    fs::File::open(&directory)?.sync_all()?;
                    Ok(())
                };
                let _ = done.send(write());
            })?;
        self.oom = Some(complete);
        Ok(())
    }

    pub(crate) fn finish_oom(&self) -> io::Result<()> {
        if let Some(complete) = &self.oom {
            complete
                .recv_timeout(std::time::Duration::from_secs(5))
                .map_err(|error| {
                    io::Error::other(format!(
                        "OOM evidence publication did not complete: {error}"
                    ))
                })??;
        }
        Ok(())
    }

    pub(crate) fn publish(&self, result: &Completion) -> io::Result<()> {
        let directory = self.directory.clone();
        let result = result.clone();
        let (done, complete) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("madrillo-state".into())
            .spawn(move || {
                let _ = done.send(Self::write(&directory, &result));
            })?;
        complete
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|error| {
                io::Error::other(format!(
                    "retained state publication did not complete: {error}"
                ))
            })?
    }

    fn write(directory: &Path, result: &Completion) -> io::Result<()> {
        let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
        serde_json::to_writer(&mut temporary, result)?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        temporary
            .persist_noclobber(directory.join("result.json"))
            .map_err(|error| error.error)?;
        #[cfg(unix)]
        fs::File::open(directory)?.sync_all()?;
        Ok(())
    }
}
