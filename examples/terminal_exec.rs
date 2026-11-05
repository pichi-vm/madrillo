#![forbid(unsafe_code)]

#[cfg(unix)]
mod qualification {
    use command_fds::{CommandFdExt, FdMapping};
    use filedescriptor::FileDescriptor;
    use nix::fcntl::{FcntlArg, FdFlag, fcntl};
    use nix::pty::{Winsize, openpty};
    use std::io::{self, IsTerminal, Read, Write};
    use std::os::fd::{AsFd, AsRawFd, IntoRawFd};
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

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

    pub(super) fn run() -> Result<()> {
        match std::env::args().nth(1).as_deref() {
            Some("launch") => launch(),
            Some("workload") => workload(),
            _ => parent(),
        }
    }

    fn launch() -> Result<()> {
        // A fresh execution of this binary is a normal Rust process, not a
        // post-fork pre_exec closure. After setup, exec replaces it in place.
        nix::unistd::setsid()?;
        rustix::process::ioctl_tiocsctty(io::stdin().as_fd())?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg("workload")
            .env("LISTEN_PID", std::process::id().to_string());
        Err(command.exec().into())
    }

    fn workload() -> Result<()> {
        let inherited = Inherited(21);
        let mut extra = FileDescriptor::dup(&inherited)?;
        nix::unistd::close(inherited)?;
        let mut bytes = Vec::new();
        extra.read_to_end(&mut bytes)?;
        let terminal =
            io::stdin().is_terminal() && io::stdout().is_terminal() && io::stderr().is_terminal();
        let size = rustix::termios::tcgetwinsize(io::stdin().as_fd())?;
        let pid = std::process::id();
        if !terminal
            || bytes != b"extra resource\0\xff"
            || std::env::var("LISTEN_PID")? != pid.to_string()
            || size.ws_row != 31
            || size.ws_col != 99
        {
            return Err("terminal/extra-resource/activation setup mismatch".into());
        }
        writeln!(
            io::stdout().lock(),
            "pid={pid};terminal=true;extra=true;activation=true;size=31x99"
        )?;
        io::stdout().lock().flush()?;
        std::process::exit(37);
    }

    fn parent() -> Result<()> {
        let size = Winsize {
            ws_row: 31,
            ws_col: 99,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let pair = openpty(Some(&size), None)?;
        fcntl(&pair.master, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))?;
        fcntl(&pair.slave, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))?;
        let (reader, mut writer) = io::pipe()?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg("launch")
            .stdin(Stdio::from(pair.slave.try_clone()?))
            .stdout(Stdio::from(pair.slave.try_clone()?))
            .stderr(Stdio::from(pair.slave.try_clone()?));
        command.fd_mappings(vec![FdMapping {
            parent_fd: reader.into(),
            child_fd: 21,
        }])?;
        let mut child = command.spawn()?;
        let pid = child.id();
        drop(command);
        drop(pair.slave);
        writer.write_all(b"extra resource\0\xff")?;
        drop(writer);
        let mut master = FileDescriptor::new(pair.master);
        master.set_non_blocking(true)?;
        let mut output = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            let mut buffer = [0; 4096];
            match master.read(&mut buffer) {
                Ok(count) => output.extend_from_slice(&buffer[..count]),
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        || error.raw_os_error() == Some(nix::libc::EIO) => {}
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error.into());
                }
            }
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline || output.len() > 65536 {
                let _ = child.kill();
                let _ = child.wait();
                return Err("terminal qualification deadline/output bound exceeded".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        // Exit can precede the final read; drain buffered terminal output.
        loop {
            let mut buffer = [0; 4096];
            match master.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => output.extend_from_slice(&buffer[..count]),
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        || error.raw_os_error() == Some(nix::libc::EIO) =>
                {
                    break;
                }
                Err(error) => return Err(error.into()),
            }
        }
        let expected = format!("pid={pid};terminal=true;extra=true;activation=true;size=31x99");
        if status.code() != Some(37) || !String::from_utf8_lossy(&output).contains(&expected) {
            return Err(format!(
                "terminal qualification failed: {status}, {}",
                String::from_utf8_lossy(&output)
            )
            .into());
        }
        writeln!(
            io::stdout().lock(),
            "PTY + inherited resource + same-PID exec + activation environment passed"
        )?;
        Ok(())
    }
}

#[cfg(unix)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    qualification::run()
}

#[cfg(not(unix))]
fn main() -> std::process::ExitCode {
    use std::io::Write;
    let _ = writeln!(
        std::io::stderr().lock(),
        "Unix-only terminal qualification; ConPTY remains unqualified"
    );
    std::process::ExitCode::FAILURE
}
