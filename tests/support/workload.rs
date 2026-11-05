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
        Some("terminal-probe") => {
            use std::io::IsTerminal;
            if !io::stdin().is_terminal()
                || !io::stdout().is_terminal()
                || !io::stderr().is_terminal()
            {
                return Err("stdio is not a terminal".into());
            }
            loop {
                let mut line = String::new();
                if io::stdin().read_line(&mut line)? == 0 {
                    return Ok(0);
                }
                match line.trim() {
                    "size" => {
                        let (terminal_size::Width(cols), terminal_size::Height(rows)) =
                            terminal_size::terminal_size().ok_or("terminal size unavailable")?;
                        writeln!(io::stdout(), "SIZE={rows}x{cols}")?;
                        io::stdout().flush()?;
                    }
                    "exit" => return Ok(37),
                    _ => {}
                }
            }
        }
        Some("emit") => {
            io::stdout().write_all(b"first\n\npartial\0\xff")?;
            io::stdout().flush()?;
            io::stderr().write_all(b"diagnostic\n")?;
            Ok(37)
        }
        Some("journal-emit") => {
            io::stdout().write_all(b"<4>priority\n<9>literal\nsplit-")?;
            io::stdout().flush()?;
            std::thread::sleep(Duration::from_millis(40));
            io::stdout().write_all(b"<2>tail\nbinary\0\xff")?;
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
        #[cfg(target_os = "linux")]
        Some("notify") => {
            let socket = std::os::unix::net::UnixDatagram::unbound()?;
            socket.connect(std::env::var_os("NOTIFY_SOCKET").ok_or("NOTIFY_SOCKET missing")?)?;
            socket.send(b"MAINPID=1\nFDSTORE=1\rREADY=1\nSTATUS=madrillo relay fixture\nWATCHDOG=1\nERRNO=0")?;
            std::thread::sleep(Duration::from_secs(15));
            Ok(0)
        }
        #[cfg(target_os = "linux")]
        Some("exec-notify") => {
            use std::os::unix::process::CommandExt;
            let error = Command::new(args.get(1).ok_or("monitor path missing")?)
                .arg("--state-dir")
                .arg(args.get(2).ok_or("state path missing")?)
                .arg("--sdnotify-socket")
                .arg(std::env::var_os("NOTIFY_SOCKET").ok_or("manager socket missing")?)
                .args(["--timeout", "10", "--"])
                .arg(std::env::current_exe()?)
                .arg("notify")
                .exec();
            Err(error.into())
        }
        #[cfg(target_os = "linux")]
        Some("orphan-parent") => {
            for _ in 0..24 {
                let mut child = Command::new(std::env::current_exe()?)
                    .args(["exit", "0"])
                    .spawn()?;
                let _ = child.try_wait()?;
            }
            Ok(0)
        }
        #[cfg(target_os = "linux")]
        Some("orphan-reap") => {
            let status = Command::new(std::env::current_exe()?)
                .arg("orphan-parent")
                .status()?;
            if !status.success() {
                return Err("orphan parent failed".into());
            }
            let parent = nix::unistd::getppid();
            let path = format!("/proc/{parent}/task/{parent}/children");
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let children = std::fs::read_to_string(&path)?;
                if children.split_whitespace().count() == 1 {
                    return Ok(0);
                }
                if Instant::now() >= deadline {
                    return Err(format!("orphans remain: {children}").into());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        #[cfg(target_os = "linux")]
        Some("placement") => {
            let expected = args
                .get(1)
                .ok_or("expected cgroup missing")?
                .to_str()
                .ok_or("invalid cgroup")?;
            let actual = std::fs::read_to_string("/proc/self/cgroup")?;
            if !actual.trim_end().ends_with(expected) {
                return Err(format!("placement {actual:?} != {expected:?}").into());
            }
            Ok(0)
        }
        #[cfg(target_os = "linux")]
        Some("bounded-oom") => {
            // Caller must install a private 32MiB cgroup limit; never stress
            // unbounded host memory even if a test setup is incorrect.
            let mut chunks = Vec::new();
            for _ in 0..64 {
                chunks.push(vec![42_u8; 1024 * 1024]);
                std::hint::black_box(&chunks);
                std::thread::sleep(Duration::from_millis(5));
            }
            Err("64MiB allocation ceiling reached without cgroup OOM".into())
        }
        Some("descendant") | Some("escaped-descendant") => {
            #[cfg(unix)]
            if args[0] == "escaped-descendant" {
                nix::unistd::setsid()?;
            }
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
        Some("tree") | Some("escaped-tree") => {
            let mode = if args[0] == "escaped-tree" {
                "escaped-descendant"
            } else {
                "descendant"
            };
            let mut child = Command::new(std::env::current_exe()?)
                .arg(mode)
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
            command.arg(std::env::var_os("MADRILLO_FIXTURE_MODE").unwrap_or_else(|| "emit".into()));
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
