use interprocess::local_socket::Stream;
use std::io;

/// Call before opening any monitor-owned handles. The caller transfers this
/// inherited connected channel to madrillo; neither stdio nor an arbitrary
/// number acquired later in the process is accepted as a transfer.
#[cfg(unix)]
pub(crate) fn control(number: usize) -> io::Result<Stream> {
    use filedescriptor::FileDescriptor;
    use std::os::{
        fd::{AsRawFd, IntoRawFd, OwnedFd},
        unix::net::UnixStream,
    };
    let number = i32::try_from(number).map_err(io::Error::other)?;
    if number < 3 {
        return Err(io::Error::other(
            "control must be an inherited descriptor distinct from stdio",
        ));
    }
    // AsRawFd is a borrowed raw descriptor, not fabricated OwnedFd/AsFd.
    // filedescriptor performs the checked OS duplication; its returned file
    // supplies actual ownership for safe conversions to standard types.
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
    let inherited = Inherited(number);
    let duplicate = FileDescriptor::dup(&inherited).map_err(io::Error::other)?;
    let owned: OwnedFd = duplicate.as_file().map_err(io::Error::other)?.into();
    let socket = UnixStream::from(owned);
    socket.peer_addr()?;
    socket.set_nonblocking(true)?;
    nix::unistd::close(inherited)?;
    Ok(interprocess::os::unix::uds_local_socket::Stream::from(socket).into())
}

#[cfg(windows)]
pub(crate) fn control(_number: usize) -> io::Result<Stream> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Windows inherited control ownership is not qualified yet",
    ))
}
