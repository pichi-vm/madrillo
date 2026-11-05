use filedescriptor::FileDescriptor;
use std::{
    fs::File,
    io::{self, Read},
};

#[derive(Debug)]
pub(crate) struct Spec {
    pub(crate) name: String,
    pub(crate) number: usize,
}

impl Spec {
    pub(crate) fn parse(values: &[String]) -> io::Result<Vec<Self>> {
        if values.len() > 128 || !values.len().is_multiple_of(2) {
            return Err(io::Error::other(
                "capture requires NAME RESOURCE pairs, at most 64",
            ));
        }
        let mut specs: Vec<Self> = Vec::new();
        for pair in values.chunks_exact(2) {
            let name = &pair[0];
            if name.is_empty()
                || name.len() > 64
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                || name == "stdout"
                || name == "stderr"
                || specs.iter().any(|spec| spec.name == *name)
            {
                return Err(io::Error::other(
                    "capture names must be unique, 1–64 ASCII letters/digits/-/_, excluding stdout and stderr",
                ));
            }
            specs.push(Self {
                name: name.clone(),
                number: pair[1].parse().map_err(io::Error::other)?,
            });
        }
        Ok(specs)
    }
}

/// Exclusive ownership of a read end, with nonblocking native operations.
/// Windows accepts synchronous byte pipes; overlapped handles fail explicitly.
pub(crate) struct Reader(File);
impl Reader {
    pub(crate) fn new(resource: FileDescriptor) -> io::Result<Self> {
        let file = resource.as_file().map_err(io::Error::other)?;
        #[cfg(unix)]
        {
            use nix::fcntl::{FcntlArg, OFlag, fcntl};
            use std::os::unix::fs::FileTypeExt;
            if !file.metadata()?.file_type().is_fifo() {
                return Err(io::Error::other("capture requires a pipe read end"));
            }
            let flags = OFlag::from_bits_truncate(fcntl(&file, FcntlArg::F_GETFL)?);
            if (flags & OFlag::O_ACCMODE) == OFlag::O_WRONLY {
                return Err(io::Error::other("capture requires a readable pipe"));
            }
            fcntl(&file, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
        }
        #[cfg(windows)]
        {
            use interprocess::os::windows::named_pipe::{RecvPipeStream, pipe_mode::Bytes};
            use std::os::windows::io::AsHandle;
            use winx::file::{FileModeInformation, query_mode_information};
            let mode = query_mode_information(file.as_handle())?;
            if !mode.intersects(
                FileModeInformation::FILE_SYNCHRONOUS_IO_ALERT
                    | FileModeInformation::FILE_SYNCHRONOUS_IO_NONALERT,
            ) {
                return Err(io::Error::other(
                    "capture requires a synchronous Windows pipe handle",
                ));
            }
            // Metadata only: Interprocess's data I/O assumes overlapped handles.
            let metadata =
                RecvPipeStream::<Bytes>::try_from(file.as_handle().try_clone_to_owned()?)
                    .map_err(|error| io::Error::other(error.to_string()))?;
            metadata.set_nonblocking(true)?;
            use system_interface::io::IoExt;
            match file.peek(&mut [0; 1]) {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {}
                Err(error) => return Err(error),
            }
        }
        Ok(Self(file))
    }
}
impl Read for Reader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        #[cfg(windows)]
        {
            use system_interface::io::IoExt;
            match self.0.peek(&mut [0; 1]) {
                Ok(0) => return Err(io::ErrorKind::WouldBlock.into()),
                Ok(_) => {}
                Err(error)
                    if error.kind() == io::ErrorKind::BrokenPipe
                        || error.raw_os_error() == Some(233) =>
                {
                    return Ok(0);
                }
                Err(error) => return Err(error),
            }
        }
        match self.0.read(bytes) {
            #[cfg(windows)]
            Err(error) if error.raw_os_error() == Some(232) => {
                Err(io::ErrorKind::WouldBlock.into())
            }
            other => other,
        }
    }
}
