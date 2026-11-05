use interprocess::local_socket::Stream;
use std::io::{self, Read, Write};

pub(crate) struct Transport {
    pub(crate) native: Stream,
    #[cfg(windows)]
    probe: std::fs::File,
}

impl Transport {
    pub(crate) fn new(native: Stream) -> io::Result<Self> {
        #[cfg(windows)]
        let probe = {
            use std::os::windows::io::AsHandle;
            let Stream::NamedPipe(stream) = &native;
            // This owned duplicate is used exclusively for PeekNamedPipe.
            // Reads/writes of the overlapped handle stay with Interprocess.
            std::fs::File::from(stream.as_handle().try_clone_to_owned()?)
        };
        Ok(Self {
            native,
            #[cfg(windows)]
            probe,
        })
    }
}

impl Read for Transport {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        #[cfg(windows)]
        {
            use system_interface::io::IoExt;
            // Interprocess 2.4.7 maps ERROR_NO_DATA through BrokenPipe to
            // EOF. With PIPE_NOWAIT that code also means an *empty live*
            // pipe. Peek preserves the distinction before attempting a read.
            match self.probe.peek(&mut [0; 1]) {
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
        self.native.read(bytes)
    }
}

impl Write for Transport {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.native.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
