//! Standard service notifications, filtered like conmon. No workload-specific API.
use std::{io, path::Path, process::Command};

#[cfg(target_os = "linux")]
pub(crate) struct Notify {
    input: std::os::unix::net::UnixDatagram,
    output: std::os::unix::net::UnixDatagram,
    path: std::path::PathBuf,
    pending: Option<Vec<u8>>,
}
#[cfg(target_os = "linux")]
impl Notify {
    pub(crate) fn open(directory: &Path, destination: &std::ffi::OsStr) -> io::Result<Self> {
        use std::os::{
            linux::net::SocketAddrExt,
            unix::{
                ffi::OsStrExt,
                net::{SocketAddr, UnixDatagram},
            },
        };
        let output = UnixDatagram::unbound()?;
        output.set_nonblocking(true)?;
        let address = if let Some(name) = destination.as_bytes().strip_prefix(b"@") {
            SocketAddr::from_abstract_name(name)?
        } else {
            SocketAddr::from_pathname(destination)?
        };
        output.connect_addr(&address)?;
        let path = std::path::absolute(directory.join("notify.sock"))?;
        let input = UnixDatagram::bind(&path)?;
        if let Err(error) = input.set_nonblocking(true) {
            let _ = std::fs::remove_file(&path);
            return Err(error);
        }
        Ok(Self {
            input,
            output,
            path,
            pending: None,
        })
    }
    pub(crate) fn configure(&self, command: &mut Command) {
        command.env("NOTIFY_SOCKET", &self.path);
    }
    /// True only after the input queue is observed empty and no send is pending.
    pub(crate) fn pump(&mut self) -> io::Result<bool> {
        for _ in 0..8 {
            if self.pending.is_none() {
                // One extra byte detects truncation without parsing a prefix.
                let mut bytes = [0; 16385];
                match self.input.recv(&mut bytes) {
                    Ok(count) if count <= 16384 => self.pending = filter(&bytes[..count]),
                    Ok(_) => return Err(io::Error::other("notification exceeded 16KiB bound")),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(true),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error),
                }
            }
            if let Some(packet) = &self.pending {
                match self.output.send(packet) {
                    Ok(count) if count == packet.len() => self.pending = None,
                    Ok(_) => return Err(io::Error::other("short notification datagram")),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(false)
    }
}
#[cfg(target_os = "linux")]
impl Drop for Notify {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
#[cfg(target_os = "linux")]
fn filter(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut packet = Vec::new();
    // NUL terminates conmon's input string. Never forward a hidden suffix.
    for line in bytes
        .split(|b| *b == 0)
        .next()?
        .split(|b| *b == b'\n' || *b == b'\r')
    {
        let permitted = matches!(
            line,
            b"READY=1" | b"RELOADING=1" | b"STOPPING=1" | b"WATCHDOG=1" | b"WATCHDOG=trigger"
        ) || [
            b"STATUS=".as_slice(),
            b"ERRNO=",
            b"BUSERROR=",
            b"MONOTONIC_USEC=",
        ]
        .iter()
        .any(|prefix| line.starts_with(prefix));
        if permitted {
            if !packet.is_empty() {
                packet.push(b'\n');
            }
            packet.extend_from_slice(line);
        }
    }
    (!packet.is_empty()).then_some(packet)
}

#[cfg(not(target_os = "linux"))]
pub(crate) struct Notify;
#[cfg(not(target_os = "linux"))]
impl Notify {
    pub(crate) fn open(_directory: &Path, _destination: &std::ffi::OsStr) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "systemd notification relay requires Linux",
        ))
    }
    pub(crate) fn configure(&self, _command: &mut Command) {}
    pub(crate) fn pump(&mut self) -> io::Result<bool> {
        Err(io::ErrorKind::Unsupported.into())
    }
}
