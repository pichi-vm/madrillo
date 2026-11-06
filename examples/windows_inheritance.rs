#![forbid(unsafe_code)]

// Qualify exact extra-handle inheritance together with stdio and native Job
// membership. This does not replace Madrillo's production Windows launcher.
#[cfg(windows)]
mod windows {
    use filedescriptor::{AsRawFileDescriptor, FileDescriptor, IntoRawFileDescriptor};
    use firehazard::{AsLocalHandle, handle, job, process};
    use std::{
        ffi::OsString,
        io::{self, Read, Write},
        sync::mpsc,
        time::{Duration, Instant},
    };

    struct Inherited(filedescriptor::RawFileDescriptor);

    impl AsRawFileDescriptor for Inherited {
        fn as_raw_file_descriptor(&self) -> filedescriptor::RawFileDescriptor {
            self.0
        }
    }

    impl IntoRawFileDescriptor for Inherited {
        fn into_raw_file_descriptor(self) -> filedescriptor::RawFileDescriptor {
            self.0
        }
    }

    struct Scope(job::OwnedHandle);

    impl Drop for Scope {
        fn drop(&mut self) {
            let _ = firehazard::terminate_job_object(&self.0, 1);
        }
    }

    impl Scope {
        fn populated(&self) -> io::Result<bool> {
            let accounting: job::object::BasicAccountingInformation =
                firehazard::query_information_job_object(&self.0)?;
            Ok(accounting.active_processes != 0)
        }

        fn wait(&self, child: &process::OwnedHandle) -> io::Result<u32> {
            let deadline = Instant::now() + Duration::from_secs(5);
            while firehazard::is_process_alive(child) || self.populated()? {
                if Instant::now() >= deadline {
                    return Err(io::Error::other("native child/Job exit deadline"));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(firehazard::get_exit_code_process(child)?)
        }
    }

    pub(super) fn run() -> io::Result<()> {
        if let Some(number) = std::env::args().nth(1) {
            let inherited = Inherited(number.parse::<usize>().map_err(io::Error::other)? as _);
            let mut extra = FileDescriptor::dup(&inherited).map_err(io::Error::other)?;
            drop(FileDescriptor::new(inherited));
            extra.write_all(b"exact extra handle\0\xff")?;
            drop(extra);
            let mut bytes = Vec::new();
            io::stdin().take(1024).read_to_end(&mut bytes)?;
            if bytes != b"standard input\0\xfe" {
                return Err(io::Error::other(
                    "stdin did not arrive through the selected handle",
                ));
            }
            io::stdout().write_all(b"standard output\0\xfd")?;
            io::stderr().write_all(b"standard error\0\xfc")?;
            std::process::exit(37);
        }

        let scope = Scope(firehazard::create_job_object_w(None, ())?);
        let (input, writer) = firehazard::pipe::create(None, 0)?;
        let (output, stdout) = firehazard::pipe::create(None, 0)?;
        let (errors, stderr) = firehazard::pipe::create(None, 0)?;
        let (extra_output, extra) = firehazard::pipe::create(None, 0)?;
        let (unlisted, withheld) = io::pipe()?;
        // Deliberately make a non-allowlisted handle inheritable. Its reader
        // must still see EOF while the child remains alive and waits on stdin.
        firehazard::set_handle_information(&withheld, handle::FLAG_INHERIT, handle::FLAG_INHERIT)?;
        let handles: [handle::Borrowed<'_>; 4] = [
            (&input).into(),
            (&stdout).into(),
            (&stderr).into(),
            (&extra).into(),
        ];
        for borrowed in &handles {
            firehazard::set_handle_information(
                *borrowed,
                handle::FLAG_INHERIT,
                handle::FLAG_INHERIT,
            )?;
        }
        let jobs: [job::Handle<'_>; 1] = [(&scope.0).into()];
        let attributes = process::ThreadAttributeList::try_from(
            [
                process::ThreadAttributeRef::handle_list(&handles),
                process::ThreadAttributeRef::job_list(&jobs),
            ]
            .as_slice(),
        )?;
        let startup = process::StartupInfoExW {
            attribute_list: Some(attributes),
            startup_info: process::StartupInfoW {
                flags: 0x100, // STARTF_USESTDHANDLES
                std_input: Some((&input).into()),
                std_output: Some((&stdout).into()),
                std_error: Some((&stderr).into()),
                ..Default::default()
            },
        };
        let executable = std::env::current_exe()?;
        let argument = OsString::from((extra.as_handle() as usize).to_string());
        let mut command_line = process::argv_to_command_line_0(&executable, [argument]);
        let child = firehazard::create_process_w(
            executable.as_os_str(),
            Some(&mut command_line),
            None,
            None,
            true,
            process::EXTENDED_STARTUPINFO_PRESENT | process::CREATE_UNICODE_ENVIRONMENT,
            process::environment::Inherit,
            (),
            &startup,
        )?;
        drop(startup);
        drop((input, stdout, stderr, extra, withheld));
        let (done, received) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = unlisted.take(1).read_to_end(&mut bytes);
            let _ = done.send(result);
        });
        if received
            .recv_timeout(Duration::from_secs(5))
            .map_err(io::Error::other)??
            != 0
        {
            return Err(io::Error::other("unlisted writer leaked into the child"));
        }
        if !scope.populated()? || !firehazard::is_process_in_job(&child.process, Some(&scope.0))? {
            return Err(io::Error::other(
                "child was not created inside the selected Job",
            ));
        }
        let mut writer = io::PipeWriter::from(writer);
        writer.write_all(b"standard input\0\xfe")?;
        drop(writer);
        if scope.wait(&child.process)? != 37 {
            return Err(io::Error::other("native child exit status changed"));
        }
        for (reader, expected) in [
            (output, b"standard output\0\xfd".as_slice()),
            (errors, b"standard error\0\xfc".as_slice()),
            (extra_output, b"exact extra handle\0\xff".as_slice()),
        ] {
            let mut bytes = Vec::new();
            io::PipeReader::from(reader)
                .take(1024)
                .read_to_end(&mut bytes)?;
            if bytes != expected {
                return Err(io::Error::other(
                    "selected output handle did not preserve bytes",
                ));
            }
        }
        println!(
            "Exact Windows handles, stdio, native Job creation and empty-Job observation passed"
        );
        Ok(())
    }
}

#[cfg(windows)]
fn main() -> std::io::Result<()> {
    windows::run()
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("Windows native qualification only");
    std::process::ExitCode::FAILURE
}
