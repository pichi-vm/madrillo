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
pub(crate) struct Terminal;
#[cfg(windows)]
impl Terminal {
    pub(crate) fn open(_rows: u16, _cols: u16) -> std::io::Result<Self> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "ConPTY with scoped spawn is not implemented yet",
        ))
    }
    pub(crate) fn resize(&self, _rows: u16, _cols: u16) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "ConPTY is not implemented yet",
        ))
    }
}
