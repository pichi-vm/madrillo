#[cfg(unix)]
mod unix {
    use nix::{
        fcntl::{FcntlArg, FdFlag, OFlag, fcntl},
        pty::{Winsize, openpty},
    };
    use std::{
        fs::File,
        io::{self, Read},
        os::fd::OwnedFd,
        process::{Command, Stdio},
        time::Duration,
    };

    pub(crate) struct Terminal {
        master: File,
        slave: Option<OwnedFd>,
    }
    pub(crate) struct Reader(File);
    impl Read for Reader {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            loop {
                match self.0.read(bytes) {
                    Err(error) if error.raw_os_error() == Some(nix::libc::EIO) => return Ok(0),
                    Err(error)
                        if error.kind() == io::ErrorKind::WouldBlock
                            || error.kind() == io::ErrorKind::Interrupted =>
                    {
                        std::thread::sleep(Duration::from_millis(2))
                    }
                    result => return result,
                }
            }
        }
    }

    impl Terminal {
        pub(crate) fn open(rows: u16, cols: u16) -> io::Result<Self> {
            let pair = openpty(
                Some(&Winsize {
                    ws_row: rows,
                    ws_col: cols,
                    ws_xpixel: 0,
                    ws_ypixel: 0,
                }),
                None,
            )?;
            fcntl(&pair.master, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))?;
            fcntl(&pair.slave, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))?;
            let flags = OFlag::from_bits_truncate(fcntl(&pair.master, FcntlArg::F_GETFL)?);
            fcntl(&pair.master, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
            Ok(Self {
                master: File::from(pair.master),
                slave: Some(pair.slave),
            })
        }
        pub(crate) fn writer(&self) -> io::Result<File> {
            self.master.try_clone()
        }
        pub(crate) fn reader(&self) -> io::Result<Reader> {
            Ok(Reader(self.master.try_clone()?))
        }
        pub(crate) fn resize(&self, rows: u16, cols: u16) -> io::Result<()> {
            if rows == 0 || cols == 0 {
                return Err(io::Error::other("terminal dimensions must be nonzero"));
            }
            rustix::termios::tcsetwinsize(
                &self.master,
                rustix::termios::Winsize {
                    ws_row: rows,
                    ws_col: cols,
                    ws_xpixel: 0,
                    ws_ypixel: 0,
                },
            )?;
            Ok(())
        }
        pub(crate) fn configure(&mut self, command: &mut Command) -> io::Result<()> {
            let slave = self
                .slave
                .take()
                .ok_or_else(|| io::Error::other("terminal already spawned"))?;
            command
                .stdin(Stdio::from(slave.try_clone()?))
                .stdout(Stdio::from(slave.try_clone()?))
                .stderr(Stdio::from(slave));
            Ok(())
        }
    }
}

#[cfg(unix)]
pub(crate) use unix::Terminal;

#[cfg(windows)]
mod windows {
    use conpty_oxide::{
        ConPtyBackend, PtyController, SessionOptions, Size,
        blocking::{Child, Command, OwnedReadHalf, OwnedWriteHalf},
    };
    use std::io;
    pub(crate) struct Terminal {
        size: Size,
        controller: Option<PtyController>,
        reader: Option<OwnedReadHalf>,
        writer: Option<OwnedWriteHalf>,
    }
    impl Terminal {
        pub(crate) fn open(rows: u16, cols: u16) -> io::Result<Self> {
            Ok(Self {
                size: Size::try_new(cols, rows).map_err(io::Error::other)?,
                controller: None,
                reader: None,
                writer: None,
            })
        }
        pub(crate) fn spawn(&mut self, original: std::process::Command) -> io::Result<Child> {
            let mut command = Command::new(original.get_program());
            command.args(original.get_args());
            if let Some(cwd) = original.get_current_dir() {
                command.current_dir(cwd);
            }
            for (key, value) in original.get_envs() {
                if let Some(value) = value {
                    command.env(key, value);
                } else {
                    command.env_remove(key);
                }
            }
            // Use the OS implementation explicitly; no adjacent DLL discovery.
            let options = SessionOptions::new()
                .size(self.size)
                .backend(ConPtyBackend::system().map_err(io::Error::other)?);
            let parts = command
                .spawn_with(options)
                .map_err(io::Error::other)?
                .into_parts();
            self.controller = Some(parts.controller);
            self.reader = Some(parts.output);
            self.writer = Some(parts.input);
            Ok(parts.child)
        }
        pub(crate) fn reader(&mut self) -> io::Result<OwnedReadHalf> {
            self.reader
                .take()
                .ok_or_else(|| io::Error::other("terminal output already consumed"))
        }
        pub(crate) fn writer(&mut self) -> io::Result<OwnedWriteHalf> {
            self.writer
                .take()
                .ok_or_else(|| io::Error::other("terminal input already consumed"))
        }
        pub(crate) fn resize(&self, rows: u16, cols: u16) -> io::Result<()> {
            self.controller
                .as_ref()
                .ok_or_else(|| io::Error::other("terminal has not started"))?
                .resize(Size::try_new(cols, rows).map_err(io::Error::other)?)
                .map_err(io::Error::other)
        }
    }
}
#[cfg(windows)]
pub(crate) use windows::Terminal;
