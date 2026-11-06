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
        #[cfg(unix)]
        Some("exit-by-signal") => {
            let signal = args
                .get(1)
                .ok_or("signal missing")?
                .to_str()
                .ok_or("invalid signal")?
                .parse::<i32>()?;
            nix::sys::signal::raise(nix::sys::signal::Signal::try_from(signal)?)?;
            Err("termination signal unexpectedly returned".into())
        }
        Some("touch") => {
            std::fs::write(args.get(1).ok_or("marker missing")?, b"invoked")?;
            Ok(0)
        }
        Some("touch-and-wait") => {
            std::fs::write(args.get(1).ok_or("marker missing")?, b"invoked")?;
            std::thread::sleep(Duration::from_secs(2));
            Ok(0)
        }
        Some("context-probe") => {
            fn bytes(value: &std::ffi::OsStr) -> Vec<u8> {
                #[cfg(unix)]
                {
                    value.as_encoded_bytes().to_vec()
                }
                #[cfg(windows)]
                {
                    use std::os::windows::ffi::OsStrExt;
                    value.encode_wide().flat_map(u16::to_le_bytes).collect()
                }
            }
            let environment =
                std::env::var_os("MADRILLO_NATIVE_VALUE").ok_or("native environment missing")?;
            let report = serde_json::json!({
                "arguments": args[2..].iter().map(|value| bytes(value)).collect::<Vec<_>>(),
                "environment": bytes(&environment),
                "cwd_marker": std::fs::read("cwd-marker")?,
            });
            std::fs::write(
                args.get(1).ok_or("report missing")?,
                serde_json::to_vec(&report)?,
            )?;
            Ok(37)
        }
        Some("held-probe") => {
            let path = args.get(1).ok_or("held file missing")?;
            #[cfg(unix)]
            {
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(path)?;
                match file.try_lock() {
                    Err(std::fs::TryLockError::WouldBlock) => {}
                    other => return Err(format!("held lock not retained: {other:?}").into()),
                }
            }
            #[cfg(windows)]
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
            {
                Err(error) if error.raw_os_error() == Some(32) => {}
                _ => return Err("exclusive held handle was not retained".into()),
            }
            Ok(37)
        }
        Some("held-hook") => {
            let state = std::path::Path::new(args.get(1).ok_or("state missing")?);
            let result: madrillo::Completion =
                serde_json::from_slice(&std::fs::read(state.join("result.json"))?)?;
            if result.leader != (madrillo::LeaderResult::Exit { code: 37 }) {
                return Err("held probe failed".into());
            }
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(args.get(2).ok_or("file missing")?)?;
            file.try_lock()?;
            std::fs::write(args.get(3).ok_or("marker missing")?, b"released")?;
            Ok(0)
        }
        #[cfg(unix)]
        Some("inherited-probe") => {
            use std::os::fd::AsRawFd;
            struct Inherited(i32);
            impl AsRawFd for Inherited {
                fn as_raw_fd(&self) -> i32 {
                    self.0
                }
            }
            let number: i32 = args
                .get(1)
                .ok_or("descriptor missing")?
                .to_str()
                .ok_or("invalid descriptor")?
                .parse()?;
            let mut pipe = filedescriptor::FileDescriptor::dup(&Inherited(number))?;
            if filedescriptor::FileDescriptor::dup(&Inherited(22)).is_ok() {
                return Err("unlisted descriptor leaked to child".into());
            }
            pipe.write_all(b"opaque\0\xff")?;
            Ok(37)
        }
        #[cfg(target_os = "linux")]
        Some("activation-probe") => {
            use std::os::fd::{AsRawFd, OwnedFd};
            use std::os::unix::net::UnixListener;
            struct Inherited(i32);
            impl AsRawFd for Inherited {
                fn as_raw_fd(&self) -> i32 {
                    self.0
                }
            }
            if std::env::var("LISTEN_PID")?.parse::<u32>()? != std::process::id()
                || std::env::var("LISTEN_FDS")? != "2"
                || std::env::var("LISTEN_FDNAMES")? != "first:second"
            {
                return Err("incorrect activation metadata".into());
            }
            for number in 3..=4 {
                let fd: OwnedFd = filedescriptor::FileDescriptor::dup(&Inherited(number))?
                    .as_file()?
                    .into();
                let listener = UnixListener::from(fd);
                let (mut connection, _) = listener.accept()?;
                let mut payload = [0; 5];
                connection.read_exact(&mut payload)?;
                connection.write_all(&payload)?;
            }
            Ok(37)
        }
        Some("exit-hook") => {
            let state = std::path::Path::new(args.get(1).ok_or("state missing")?);
            let result: madrillo::Completion =
                serde_json::from_slice(&std::fs::read(state.join("result.json"))?)?;
            if result.leader != (madrillo::LeaderResult::Exit { code: 37 }) {
                return Err("hook did not see retained workload exit".into());
            }
            for name in [
                "LISTEN_PID",
                "LISTEN_PIDFDID",
                "LISTEN_FDS",
                "LISTEN_FDNAMES",
                "NOTIFY_SOCKET",
            ] {
                if std::env::var_os(name).is_some() {
                    return Err(format!("hook inherited workload environment {name}").into());
                }
            }
            let mut output = Vec::new();
            for arg in &args[4..] {
                #[cfg(unix)]
                output.extend_from_slice(arg.as_encoded_bytes());
                #[cfg(windows)]
                {
                    use std::os::windows::ffi::OsStrExt;
                    output.extend(arg.encode_wide().flat_map(u16::to_le_bytes));
                }
                output.push(b'\n');
            }
            std::fs::write(args.get(2).ok_or("marker missing")?, output)?;
            #[cfg(target_os = "linux")]
            std::fs::write(
                state.join("hook-oom-score"),
                std::fs::read("/proc/self/oom_score_adj")?,
            )?;
            Ok(args
                .get(3)
                .ok_or("hook status missing")?
                .to_str()
                .ok_or("invalid status")?
                .parse()?)
        }
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
        Some("echo") | Some("burst-echo") => {
            if args[0] == "burst-echo" {
                io::stdout().write_all(&[b'x'; 256 * 1024])?;
                io::stdout().flush()?;
            }
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
        Some("exec-with-oom-score") => {
            use std::os::unix::process::CommandExt;
            std::fs::write(
                "/proc/self/oom_score_adj",
                args.get(1).ok_or("score missing")?.as_encoded_bytes(),
            )?;
            Err(Command::new(args.get(2).ok_or("program missing")?)
                .args(&args[3..])
                .exec()
                .into())
        }
        #[cfg(target_os = "linux")]
        Some("oom-probe") => {
            let parent = nix::unistd::getppid();
            let report = serde_json::json!({
                "monitor": std::fs::read_to_string(format!("/proc/{parent}/oom_score_adj"))?.trim().parse::<i32>()?,
                "workload": std::fs::read_to_string("/proc/self/oom_score_adj")?.trim().parse::<i32>()?,
            });
            std::fs::write(
                args.get(1).ok_or("report missing")?,
                serde_json::to_vec(&report)?,
            )?;
            Ok(37)
        }
        #[cfg(target_os = "linux")]
        Some(mode @ ("notify" | "notify-oversized")) => {
            let socket = std::os::unix::net::UnixDatagram::unbound()?;
            socket.connect(std::env::var_os("NOTIFY_SOCKET").ok_or("NOTIFY_SOCKET missing")?)?;
            if mode == "notify-oversized" {
                let mut packet = b"READY=1\nSTATUS=".to_vec();
                packet.resize(16385, b'x');
                socket.send(&packet)?;
            } else {
                socket.send(b"MAINPID=1\nFDSTORE=1\rREADY=1\nSTATUS=madrillo relay fixture\nWATCHDOG=1\nERRNO=0")?;
            }
            std::io::stdout().write_all(b"notification sent\n")?;
            std::io::stdout().flush()?;
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
        Some("branch")
        | Some("escaped-branch")
        | Some("deep-tree")
        | Some("daemon-tree")
        | Some("escaped-leader")
        | Some("nested-cgroup-tree")
        | Some("waiting-tree")
        | Some("waiting-daemon-tree") => {
            let path = args.get(1).ok_or("heartbeat path missing")?;
            #[cfg(unix)]
            if args[0] == "escaped-branch" {
                nix::unistd::setsid()?;
            }
            #[cfg(target_os = "linux")]
            if args[0] == "escaped-leader" {
                let parent_group = nix::unistd::getpgid(Some(nix::unistd::getppid()))?;
                nix::unistd::setpgid(nix::unistd::Pid::from_raw(0), parent_group)?;
            }
            #[cfg(target_os = "linux")]
            if args[0] == "nested-cgroup-tree" {
                let nested =
                    std::path::Path::new(args.get(2).ok_or("cgroup path missing")?).join("nested");
                std::fs::create_dir(&nested)?;
                std::fs::write(nested.join("cgroup.procs"), b"0")?;
            }
            let mut child = Command::new(std::env::current_exe()?)
                .arg(match args[0].to_str() {
                    Some("branch") => "descendant",
                    Some("escaped-branch") => "escaped-descendant",
                    Some(
                        "daemon-tree"
                        | "waiting-daemon-tree"
                        | "escaped-leader"
                        | "nested-cgroup-tree",
                    ) => "escaped-branch",
                    _ => "branch",
                })
                .arg(path)
                .spawn()?;
            let deadline = Instant::now() + Duration::from_secs(3);
            while std::fs::metadata(path).map_or(true, |metadata| metadata.len() == 0) {
                if Instant::now() >= deadline {
                    child.kill()?;
                    child.wait()?;
                    return Err("grandchild did not become ready".into());
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            if args[0] != "deep-tree" && args[0] != "daemon-tree" && args[0] != "nested-cgroup-tree"
            {
                child.wait()?;
            }
            Ok(37)
        }
        Some("racing-tree") | Some("racing-escaped-tree") => {
            let mut children = std::collections::VecDeque::<std::process::Child>::new();
            let deadline = Instant::now() + Duration::from_secs(15);
            while Instant::now() < deadline {
                children.push_back(
                    Command::new(std::env::current_exe()?)
                        .arg(if args[0] == "racing-escaped-tree" {
                            "escaped-descendant"
                        } else {
                            "descendant"
                        })
                        .arg(args.get(1).ok_or("heartbeat path missing")?)
                        .spawn()?,
                );
                if children.len() > 16 {
                    let mut child = children.pop_front().ok_or("missing child")?;
                    child.kill()?;
                    child.wait()?;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            for mut child in children {
                child.kill()?;
                child.wait()?;
            }
            Ok(37)
        }
        Some("tree") | Some("escaped-tree") => {
            let mode = if args[0] == "escaped-tree" {
                "escaped-descendant"
            } else {
                "descendant"
            };
            let mut command = Command::new(std::env::current_exe()?);
            command
                .arg(mode)
                .arg(args.get(1).ok_or("heartbeat path missing")?);
            if args[0] == "escaped-tree" {
                command
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null());
            }
            let mut child = command.spawn()?;
            if args[0] == "escaped-tree" {
                println!("{}", child.id());
            }
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
