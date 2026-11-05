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
pub(crate) fn control(number: usize) -> io::Result<Stream> {
    use filedescriptor::FileDescriptor;
    use interprocess::local_socket::prelude::*;
    use std::os::windows::io::{AsRawHandle, IntoRawHandle, OwnedHandle, RawHandle};
    if number < 4
        || number > isize::MAX as usize
        || [
            io::stdin().as_raw_handle(),
            io::stdout().as_raw_handle(),
            io::stderr().as_raw_handle(),
        ]
        .iter()
        .any(|handle| *handle as usize == number)
    {
        return Err(io::Error::other(
            "control must be an inherited handle distinct from stdio",
        ));
    }
    struct Inherited(RawHandle);
    impl AsRawHandle for Inherited {
        fn as_raw_handle(&self) -> RawHandle {
            self.0
        }
    }
    impl IntoRawHandle for Inherited {
        fn into_raw_handle(self) -> RawHandle {
            self.0
        }
    }
    let inherited = Inherited(number as RawHandle);
    // DuplicateHandle validates the supplied resource and clears inheritance.
    // Retain the original until the duplicate has passed pipe validation.
    let duplicate = FileDescriptor::dup(&inherited).map_err(io::Error::other)?;
    let owned: OwnedHandle = duplicate.as_file().map_err(io::Error::other)?.into();
    let stream = interprocess::os::windows::named_pipe::local_socket::Stream::try_from(owned)
        .map_err(|error| io::Error::other(error.to_string()))?;
    stream.inner().peer_process_id()?;
    stream.set_nonblocking(true)?;
    // The CLI transfers this original handle. Closing only the duplicate would
    // leave another console/control owner inherited by the workload.
    drop(FileDescriptor::new(inherited));
    Ok(stream.into())
}
