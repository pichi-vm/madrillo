use madrillo::Completion;
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

pub(crate) struct State {
    directory: PathBuf,
}

impl State {
    pub(crate) fn create(path: &Path) -> io::Result<Self> {
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        // Refuse reuse: a retained result must never belong to a different run.
        builder.create(path)?;
        Ok(Self {
            directory: path.to_owned(),
        })
    }

    pub(crate) fn publish(&self, result: &Completion) -> io::Result<()> {
        let mut temporary = tempfile::NamedTempFile::new_in(&self.directory)?;
        serde_json::to_writer(&mut temporary, result)?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        temporary
            .persist_noclobber(self.directory.join("result.json"))
            .map_err(|error| error.error)?;
        #[cfg(unix)]
        fs::File::open(&self.directory)?.sync_all()?;
        Ok(())
    }
}
