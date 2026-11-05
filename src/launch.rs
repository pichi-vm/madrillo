//! Fresh-process Unix setup: no unsafe post-fork callback or persistent helper.
use command_fds::{CommandFdExt, FdMapping};
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use std::{
    ffi::OsString,
    io::{self, Read, Write},
    os::{fd::AsFd, unix::process::CommandExt},
    process::{Child, Command},
    time::{Duration, Instant},
};

pub(crate) fn command(args: &[OsString], terminal: bool, scoped: bool) -> io::Result<Command> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("__madrillo_exec")
        .arg(if terminal { "tty" } else { "pipe" })
        .arg(if scoped { "cgroup" } else { "group" })
        .args(args);
    Ok(command)
}

pub(crate) fn spawn(mut command: Command, membership: Option<std::fs::File>) -> io::Result<Child> {
    // Equivalent to std's exec-error pipe: CLOEXEC closes it at successful exec.
    let (mut errors, report) = io::pipe()?;
    let flags = OFlag::from_bits_truncate(fcntl(&errors, FcntlArg::F_GETFL)?);
    fcntl(&errors, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
    let mut mappings = vec![FdMapping {
        parent_fd: report.into(),
        child_fd: 21,
    }];
    if let Some(file) = membership {
        mappings.push(FdMapping {
            parent_fd: file.into(),
            child_fd: 22,
        });
    }
    command.fd_mappings(mappings).map_err(io::Error::other)?;
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
                message = b"setup error exceeded its size bound".to_vec();
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
            message = b"setup/exec deadline expired".to_vec();
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let _ = child.kill();
    // A stuck kernel wait must not indefinitely hold launch failure reporting.
    let _ = std::thread::Builder::new()
        .name("madrillo-failed-launch".into())
        .spawn(move || {
            let _ = child.wait();
        });
    Err(io::Error::other(format!(
        "workload launch: {}",
        String::from_utf8_lossy(&message)
    )))
}

fn adopt(fd: i32) -> io::Result<filedescriptor::FileDescriptor> {
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
    let owned = filedescriptor::FileDescriptor::dup(&Inherited(fd)).map_err(io::Error::other)?;
    nix::unistd::close(Inherited(fd))?;
    Ok(owned)
}

pub(crate) fn exec() -> io::Result<()> {
    let mut report = adopt(21)?;
    let error = (|| -> io::Result<()> {
        let mut args = std::env::args_os().skip(2);
        let terminal = args.next().is_some_and(|arg| arg == "tty");
        let scoped = args.next().is_some_and(|arg| arg == "cgroup");
        if scoped {
            let mut membership = adopt(22)?;
            membership.write_all(b"0")?;
        }
        if terminal {
            nix::unistd::setsid()?;
            rustix::process::ioctl_tiocsctty(io::stdin().as_fd())?;
        } else {
            nix::unistd::setpgid(nix::unistd::Pid::from_raw(0), nix::unistd::Pid::from_raw(0))?;
        }
        let program = args
            .next()
            .ok_or_else(|| io::Error::other("launcher omitted program"))?;
        Err(Command::new(program).args(args).exec())
    })();
    if let Err(error) = &error {
        let message = error.to_string();
        let bytes = message.as_bytes();
        let _ = report.write_all(&bytes[..bytes.len().min(8192)]);
    }
    error
}
