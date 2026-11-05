#![forbid(unsafe_code)]
//! Native observation of console loss with unread input. Not full conformance.
#[cfg(unix)]
use std::io::Read;
use std::io::{self, Write};

#[cfg(unix)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use nix::{
        fcntl::{FcntlArg, OFlag, fcntl},
        poll::{PollFd, PollFlags, poll},
    };
    use std::os::fd::AsFd;
    let (mut reader, mut writer) = io::pipe()?;
    let flags = OFlag::from_bits_truncate(fcntl(&writer, FcntlArg::F_GETFL)?);
    fcntl(&writer, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
    let mut filled = 0;
    for _ in 0..256 {
        match writer.write(&[b'x'; 8192]) {
            Ok(0) => break,
            Ok(count) => filled += count,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) => return Err(error.into()),
        }
    }
    assert!(filled > 0);
    drop(writer);
    let mut descriptors = [PollFd::new(
        reader.as_fd(),
        PollFlags::POLLIN | PollFlags::POLLHUP,
    )];
    poll(&mut descriptors, 0u8)?;
    assert!(
        descriptors[0]
            .revents()
            .ok_or("missing readiness")?
            .contains(PollFlags::POLLHUP)
    );
    let mut data = Vec::new();
    reader.read_to_end(&mut data)?;
    assert_eq!(data.len(), filled);
    println!("Unix console loss: POLLHUP observed without consuming {filled} queued bytes");
    Ok(())
}

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use interprocess::os::windows::named_pipe::{PipeListenerOptions, pipe_mode::Bytes};
    use system_interface::io::IoExt;
    let path_text = format!(r"\\.\pipe\madrillo-liveness-{}", std::process::id());
    let path = widestring::U16CString::from_str(&path_text)?;
    let listener = PipeListenerOptions::new()
        .path(path.as_ucstr())
        .nonblocking(true)
        .output_buffer_size_hint(64 * 1024)
        .create_send_only::<Bytes>()?;
    let reader = std::fs::OpenOptions::new().read(true).open(&path_text)?;
    let mut writer = listener.accept()?;
    let mut filled = 0;
    for _ in 0..256 {
        match writer.write(&[b'x'; 8192]) {
            Ok(0) => break,
            Ok(count) => filled += count,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) => return Err(error.into()),
        }
    }
    assert!(filled > 0);
    assert_eq!(reader.peek(&mut [0; 1])?, 1);
    writer.assume_flushed();
    drop(writer);
    drop(listener);
    let after_close = reader.peek(&mut [0; 1]);
    println!(
        "Windows console loss: {filled} unread bytes; PeekNamedPipe immediately after server close: {after_close:?}"
    );
    let mut received = 0;
    loop {
        match reader.read(&mut [0; 8192]) {
            Ok(0) => break,
            Ok(count) => received += count,
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => break,
            Err(error) => return Err(error.into()),
        }
    }
    println!(
        "Windows console loss: {received} bytes readable after close; final peek: {:?}",
        reader.peek(&mut [0; 1])
    );
    Ok(())
}
