#![forbid(unsafe_code)]

#[cfg(unix)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use command_fds::{CommandFdExt, FdMapping};
    use filedescriptor::FileDescriptor;
    use std::io::{Read, Write};
    use std::os::fd::{AsRawFd, IntoRawFd};
    use std::process::{Command, Stdio};

    // This experiment uses a borrowed raw number only to duplicate a descriptor.
    // It does not fabricate an OwnedFd or call an unsafe receiver initializer.
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

    if std::env::args_os().nth(1).is_some() {
        let inherited = Inherited(21);
        let mut input = FileDescriptor::dup(&inherited)?;
        nix::unistd::close(inherited)?;
        let mut bytes = Vec::new();
        input.read_to_end(&mut bytes)?;
        std::io::stdout().lock().write_all(&bytes)?;
        return Ok(());
    }

    let (reader, mut writer) = std::io::pipe()?;
    let mut command = Command::new(std::env::current_exe()?);
    command.arg("receive").stdout(Stdio::piped());
    command.fd_mappings(vec![FdMapping {
        parent_fd: reader.into(),
        child_fd: 21,
    }])?;
    let child = command.spawn()?;
    drop(command);
    writer.write_all(b"inherited pipe bytes\0\xff")?;
    drop(writer);
    let output = child.wait_with_output()?;
    if !output.status.success() || output.stdout != b"inherited pipe bytes\0\xff" {
        return Err("inherited pipe experiment failed".into());
    }
    writeln!(
        std::io::stdout().lock(),
        "Linux/Unix inherited-pipe duplication and original closure passed"
    )?;
    Ok(())
}

#[cfg(not(unix))]
fn main() -> std::process::ExitCode {
    use std::io::Write;
    let _ = writeln!(
        std::io::stderr().lock(),
        "Unix-only qualification experiment; Windows remains unqualified"
    );
    std::process::ExitCode::FAILURE
}
