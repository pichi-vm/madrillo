use filedescriptor::{
    AsRawFileDescriptor, FileDescriptor, IntoRawFileDescriptor, RawFileDescriptor,
};
use interprocess::local_socket::Stream;
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
};

struct Inherited(RawFileDescriptor);
impl AsRawFileDescriptor for Inherited {
    fn as_raw_file_descriptor(&self) -> RawFileDescriptor {
        self.0
    }
}
impl IntoRawFileDescriptor for Inherited {
    fn into_raw_file_descriptor(self) -> RawFileDescriptor {
        self.0
    }
}

fn inherited(number: usize) -> io::Result<Inherited> {
    #[cfg(unix)]
    {
        let number = i32::try_from(number).map_err(io::Error::other)?;
        if number < 3 {
            return Err(io::Error::other(
                "inherited resources must be distinct from stdio",
            ));
        }
        Ok(Inherited(number))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
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
                "inherited resources must be distinct from stdio",
            ));
        }
        Ok(Inherited(number as RawFileDescriptor))
    }
}

/// Duplicate and validate the complete transfer before opening other resources.
/// A new duplicate must not accidentally make a later invalid number valid.
/// Original transfers close only after all checked duplicates exist.
pub(crate) fn adopt(numbers: &[usize]) -> io::Result<BTreeMap<usize, FileDescriptor>> {
    if numbers.len() > 64 {
        return Err(io::Error::other(
            "at most 64 inherited resources are supported",
        ));
    }
    let unique: BTreeSet<_> = numbers.iter().copied().collect();
    if unique.len() != numbers.len() {
        return Err(io::Error::other("each resource must have exactly one role"));
    }
    let originals = numbers
        .iter()
        .map(|number| inherited(*number))
        .collect::<io::Result<Vec<_>>>()?;
    let mut owned = BTreeMap::new();
    for (number, original) in numbers.iter().zip(&originals) {
        let duplicate = FileDescriptor::dup(original).map_err(io::Error::other)?;
        if unique.contains(&(duplicate.as_raw_file_descriptor() as usize)) {
            return Err(io::Error::other(
                "a claimed inherited resource was not open at adoption",
            ));
        }
        owned.insert(*number, duplicate);
    }
    for original in originals {
        drop(FileDescriptor::new(original));
    }
    #[cfg(unix)]
    close_unlisted(
        &owned
            .values()
            .map(|resource| resource.as_raw_file_descriptor())
            .collect::<Vec<_>>(),
    )?;
    Ok(owned)
}

#[cfg(unix)]
pub(crate) fn control(resource: FileDescriptor) -> io::Result<Stream> {
    use std::os::{fd::OwnedFd, unix::net::UnixStream};
    let owned: OwnedFd = resource.as_file().map_err(io::Error::other)?.into();
    let socket = UnixStream::from(owned);
    socket.peer_addr()?;
    socket.set_nonblocking(true)?;
    Ok(interprocess::os::unix::uds_local_socket::Stream::from(socket).into())
}

#[cfg(windows)]
pub(crate) fn control(resource: FileDescriptor) -> io::Result<Stream> {
    use interprocess::local_socket::prelude::*;
    use std::os::windows::io::OwnedHandle;
    let owned: OwnedHandle = resource.as_file().map_err(io::Error::other)?.into();
    let stream = interprocess::os::windows::named_pipe::local_socket::Stream::try_from(owned)
        .map_err(|error| io::Error::other(error.to_string()))?;
    stream.inner().peer_process_id()?;
    stream.set_nonblocking(true)?;
    Ok(stream.into())
}

#[cfg(unix)]
pub(crate) fn close_unlisted(keep: &[i32]) -> io::Result<()> {
    // This is a fresh, single-threaded process. Collect and close the directory
    // before closing unlisted descriptors; no scan descriptor leaks to exec.
    #[cfg(target_os = "linux")]
    let directory = "/proc/self/fd";
    #[cfg(not(target_os = "linux"))]
    let directory = "/dev/fd";
    let numbers = std::fs::read_dir(directory)?
        .map(|entry| {
            let name = entry?.file_name();
            name.to_str()
                .ok_or_else(|| io::Error::other("invalid descriptor entry"))?
                .parse::<i32>()
                .map_err(io::Error::other)
        })
        .collect::<io::Result<Vec<_>>>()?;
    for number in numbers
        .into_iter()
        .filter(|number| *number >= 3 && !keep.contains(number))
    {
        // The directory itself has already closed, so EBADF is expected for it.
        match nix::unistd::close(number) {
            Ok(()) | Err(nix::errno::Errno::EBADF) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
