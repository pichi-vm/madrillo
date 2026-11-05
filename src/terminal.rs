#[cfg(unix)]
mod unix {
    use command_fds::{CommandFdExt, FdMapping};
    use nix::{
        fcntl::{FcntlArg, FdFlag, OFlag, fcntl},
        pty::{Winsize, openpty},
    };
    use std::{
        fs::File,
        io::{self, Read, Write},
        os::{
            fd::{AsFd, OwnedFd},
            unix::process::CommandExt,
        },
        process::{Child, Command, Stdio},
        time::{Duration, Instant},
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
        pub(crate) fn spawn(&mut self, original: Command) -> io::Result<Child> {
            let mut command = Command::new(std::env::current_exe()?);
            command
                .arg("__madrillo_terminal_exec")
                .arg(original.get_program())
                .args(original.get_args());
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
            let slave = self
                .slave
                .take()
                .ok_or_else(|| io::Error::other("terminal already spawned"))?;
            command
                .stdin(Stdio::from(slave.try_clone()?))
                .stdout(Stdio::from(slave.try_clone()?))
                .stderr(Stdio::from(slave));
            // Like std's exec-error pipe, EOF means successful exec. This is
            // internal launch error reporting, not a client startup handshake.
            let (mut errors, report) = io::pipe()?;
            let flags = OFlag::from_bits_truncate(fcntl(&errors, FcntlArg::F_GETFL)?);
            fcntl(&errors, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
            command
                .fd_mappings(vec![FdMapping {
                    parent_fd: report.into(),
                    child_fd: 21,
                }])
                .map_err(io::Error::other)?;
            let mut child = command.spawn()?;
            drop(command);
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut message = Vec::new();
            let mut buffer = [0; 1024];
            loop {
                match errors.read(&mut buffer) {
                    Ok(0) if message.is_empty() => return Ok(child),
                    Ok(0) => break,
                    Ok(count) if message.len().saturating_add(count) <= 8192 => {
                        message.extend_from_slice(&buffer[..count])
                    }
                    Ok(_) => {
                        message = b"terminal setup error exceeded its size bound".to_vec();
                        break;
                    }
                    Err(error)
                        if error.kind() == io::ErrorKind::WouldBlock
                            || error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) => {
                        message = error.to_string().into_bytes();
                        break;
                    }
                }
                if Instant::now() >= deadline {
                    message = b"terminal setup/exec deadline expired".to_vec();
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            let _ = child.kill();
            let _ = child.wait();
            Err(io::Error::other(format!(
                "terminal workload launch: {}",
                String::from_utf8_lossy(&message)
            )))
        }
    }

    pub(crate) fn exec() -> io::Result<()> {
        use filedescriptor::FileDescriptor;
        use std::os::fd::{AsRawFd, IntoRawFd};
        struct Inherited(i32);
        impl AsRawFd for Inherited {
            fn as_raw_fd(&self) -> i32 {
                self.0
            }
        }
        impl IntoRawFd for Inherited {
            fn into_raw_fd(self) -> i32 {
                self.0
            }
        }
        let mut report = FileDescriptor::dup(&Inherited(21)).map_err(io::Error::other)?;
        nix::unistd::close(Inherited(21))?;
        let error = (|| -> io::Result<()> {
            nix::unistd::setsid()?;
            rustix::process::ioctl_tiocsctty(io::stdin().as_fd())?;
            let mut args = std::env::args_os().skip(2);
            let program = args
                .next()
                .ok_or_else(|| io::Error::other("terminal launcher omitted program"))?;
            let mut command = Command::new(program);
            command.args(args);
            Err(command.exec())
        })();
        if let Err(error) = &error {
            let message = error.to_string();
            let bytes = message.as_bytes();
            let _ = report.write_all(&bytes[..bytes.len().min(8192)]);
        }
        error
    }
}

#[cfg(unix)]
pub(crate) use unix::{Terminal, exec};

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
