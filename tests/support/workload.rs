#![forbid(unsafe_code)]
use std::{
    io::{self, Read, Write},
    process::{Command, ExitCode},
    time::{Duration, Instant},
};

fn main() -> ExitCode {
    match run() {
        Ok(code) => {
            std::process::exit(code);
        }
        Err(error) => {
            let _ = writeln!(io::stderr(), "fixture: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<i32, Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    match args.first().and_then(|s| s.to_str()) {
        Some("emit") => {
            io::stdout().write_all(b"first\n\npartial\0\xff")?;
            io::stdout().flush()?;
            io::stderr().write_all(b"diagnostic\n")?;
            Ok(37)
        }
        Some("sleep") => {
            std::thread::sleep(Duration::from_secs(20));
            Ok(0)
        }
        Some("echo") => {
            let mut buffer = [0; 8192];
            loop {
                let count = io::stdin().read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                io::stdout().write_all(&buffer[..count])?;
                io::stdout().flush()?;
            }
            Ok(0)
        }
        Some("flood") => {
            let chunk = [b'x'; 8192];
            loop {
                io::stdout().write_all(&chunk)?;
            }
        }
        Some("stdin-survival") => {
            // With no client, the monitor must keep the pipe writer open.
            let mut byte = [0; 1];
            if io::stdin().read(&mut byte)? == 0 {
                return Ok(91);
            }
            Ok(0)
        }
        Some("exit") => Ok(args
            .get(1)
            .ok_or("exit code missing")?
            .to_str()
            .ok_or("exit code invalid")?
            .parse()?),
        Some("args") => {
            for arg in args.iter().skip(1) {
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStrExt;
                    io::stdout().write_all(arg.as_bytes())?;
                }
                #[cfg(windows)]
                {
                    use std::os::windows::ffi::OsStrExt;
                    for unit in arg.encode_wide() {
                        io::stdout().write_all(&unit.to_le_bytes())?;
                    }
                }
                io::stdout().write_all(b"\n")?;
            }
            Ok(0)
        }
        Some("descendant") => {
            let path = args.get(1).ok_or("heartbeat path missing")?;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            let end = Instant::now() + Duration::from_secs(15);
            while Instant::now() < end {
                file.write_all(b".")?;
                file.flush()?;
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(0)
        }
        Some("tree") => {
            let mut child = Command::new(std::env::current_exe()?)
                .arg("descendant")
                .arg(args.get(1).ok_or("heartbeat path missing")?)
                .spawn()?;
            std::thread::sleep(Duration::from_secs(1));
            // Intentionally leave a bounded descendant behind; the monitor
            // must terminate it as part of handling the leader's exit.
            let _ = child.try_wait()?;
            Ok(37)
        }
        Some("create") => {
            let index = args
                .iter()
                .position(|s| s == "--pid-file")
                .ok_or("conmon omitted --pid-file")?;
            let path = args.get(index + 1).ok_or("conmon omitted pid path")?;
            let mut command = Command::new(std::env::current_exe()?);
            command.arg("emit");
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                command.process_group(0);
            }
            let child = command.spawn()?;
            std::fs::write(path, child.id().to_string())?;
            Ok(0)
        }
        _ => Err("unknown fixture mode".into()),
    }
}
