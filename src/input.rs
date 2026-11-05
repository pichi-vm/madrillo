use std::{
    io::{self, Write},
    process::ChildStdin,
};

const MAX_INPUT_BYTES: usize = 64 * 1024;

pub(crate) struct Input {
    #[cfg(unix)]
    writer: Option<ChildStdin>,
    #[cfg(windows)]
    writer: Option<
        interprocess::os::windows::named_pipe::SendPipeStream<
            interprocess::os::windows::named_pipe::pipe_mode::Bytes,
        >,
    >,
    pending: Vec<u8>,
    written: usize,
    closing: bool,
}

impl Input {
    pub(crate) fn new(writer: Option<ChildStdin>) -> io::Result<Self> {
        #[cfg(unix)]
        if let Some(writer) = &writer {
            use nix::fcntl::{FcntlArg, OFlag, fcntl};
            let flags = OFlag::from_bits_truncate(fcntl(writer, FcntlArg::F_GETFL)?);
            fcntl(writer, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
        }
        #[cfg(windows)]
        let writer = writer
            .map(|writer| -> io::Result<_> {
                use interprocess::os::windows::named_pipe::{SendPipeStream, pipe_mode::Bytes};
                let owned: std::os::windows::io::OwnedHandle = writer.into();
                let pipe = SendPipeStream::<Bytes>::try_from(owned)
                    .map_err(|error| io::Error::other(error.to_string()))?;
                pipe.set_nonblocking(true)?;
                Ok(pipe)
            })
            .transpose()?;
        Ok(Self {
            writer,
            pending: Vec::new(),
            written: 0,
            closing: false,
        })
    }
    pub(crate) fn enqueue(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.writer.is_none() || self.closing {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "child input is closed",
            ));
        }
        if self
            .pending
            .len()
            .saturating_sub(self.written)
            .saturating_add(bytes.len())
            > MAX_INPUT_BYTES
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "child input queue is full",
            ));
        }
        if self.written > 0 {
            self.pending.drain(..self.written);
            self.written = 0;
        }
        self.pending
            .try_reserve(bytes.len())
            .map_err(|_| io::Error::other("input allocation failed"))?;
        self.pending.extend_from_slice(bytes);
        Ok(())
    }
    pub(crate) fn flush(&mut self) -> io::Result<()> {
        if self.pending.len() == self.written {
            if self.closing {
                self.close();
            }
            return Ok(());
        }
        if let Some(writer) = &mut self.writer {
            match writer.write(&self.pending[self.written..]) {
                Ok(count) => self.written += count,
                Err(e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => {
                    self.close();
                    return Err(error);
                }
            }
        }
        if self.pending.len() == self.written {
            self.pending.clear();
            self.written = 0;
            if self.closing {
                self.close();
            }
        }
        Ok(())
    }
    pub(crate) fn close(&mut self) {
        self.writer = None;
        self.pending.clear();
        self.written = 0;
    }
    pub(crate) fn close_after_flush(&mut self) {
        self.closing = true;
    }
}
