use std::{
    io::{self, Write},
    process::Stdio,
};

const MAX_INPUT_BYTES: usize = 64 * 1024;

pub(crate) struct Input {
    #[cfg(unix)]
    writer: Option<std::fs::File>,
    #[cfg(windows)]
    writer: Option<WindowsWriter>,
    pending: Vec<u8>,
    written: usize,
    closing: bool,
}

impl Input {
    #[cfg(unix)]
    pub(crate) fn terminal(file: std::fs::File) -> Self {
        Self {
            writer: Some(file),
            pending: Vec::new(),
            written: 0,
            closing: false,
        }
    }
    pub(crate) fn disabled() -> Self {
        Self {
            writer: None,
            pending: Vec::new(),
            written: 0,
            closing: false,
        }
    }
    pub(crate) fn pipe() -> io::Result<(Self, Stdio)> {
        #[cfg(unix)]
        let (writer, input) = {
            use nix::fcntl::{FcntlArg, OFlag, fcntl};
            let (reader, writer) = io::pipe()?;
            let flags = OFlag::from_bits_truncate(fcntl(&writer, FcntlArg::F_GETFL)?);
            fcntl(&writer, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
            let owned: std::os::fd::OwnedFd = writer.into();
            (std::fs::File::from(owned), Stdio::from(reader))
        };
        #[cfg(windows)]
        let (writer, input) = {
            use interprocess::os::windows::{
                named_pipe::{PipeListenerOptions, pipe_mode::Bytes},
                security_descriptor::SecurityDescriptor,
            };
            let path_text = format!(r"\\.\pipe\madrillo-input-{}", std::process::id());
            let path = widestring::U16CString::from_str(&path_text).map_err(io::Error::other)?;
            let sddl =
                widestring::U16CString::from_str("D:P(A;;GA;;;OW)").map_err(io::Error::other)?;
            let listener = PipeListenerOptions::new()
                .path(path.as_ucstr())
                .security_descriptor(Some(SecurityDescriptor::deserialize(&sddl)?))
                .nonblocking(true)
                .create_send_only::<Bytes>()?;
            // The child receives ordinary synchronous stdin, even though the
            // monitor's server handle supports overlapped nonblocking writes.
            let reader = std::fs::OpenOptions::new().read(true).open(path_text)?;
            let writer = WindowsWriter {
                pipe: listener.accept()?,
            };
            (writer, Stdio::from(reader))
        };
        Ok((
            Self {
                writer: Some(writer),
                pending: Vec::new(),
                written: 0,
                closing: false,
            },
            input,
        ))
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
                self.finish_close()?;
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
                self.finish_close()?;
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
    fn finish_close(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        {
            self.close();
        }
        #[cfg(windows)]
        if let Some(writer) = self.writer.take() {
            // One bounded worker for this input's sole EOF transition. Windows
            // must flush kernel pipe bytes before closing; a non-reading child
            // must not block deadline or control processing on the monitor.
            std::thread::Builder::new()
                .name("madrillo-input-close".into())
                .spawn(move || {
                    let _ = writer.pipe.flush();
                })?;
        }
        Ok(())
    }
}

#[cfg(windows)]
struct WindowsWriter {
    pipe: interprocess::os::windows::named_pipe::SendPipeStream<
        interprocess::os::windows::named_pipe::pipe_mode::Bytes,
    >,
}
#[cfg(windows)]
impl Write for WindowsWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.pipe.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.pipe.flush()
    }
}
#[cfg(windows)]
impl Drop for WindowsWriter {
    fn drop(&mut self) {
        // All drain waits are explicitly owned above. Do not let dependency
        // drop behavior accumulate background workers for stalled consumers.
        self.pipe.assume_flushed();
    }
}
