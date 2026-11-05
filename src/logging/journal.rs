//! Conmon journal fields and bounded partial lines over systemd's native protocol.
use super::cri::Stream;
use std::io;

pub(crate) struct Metadata<'a> {
    pub(crate) id: &'a str,
    pub(crate) name: Option<&'a str>,
    pub(crate) tag: Option<&'a str>,
    pub(crate) labels: &'a [String],
    pub(crate) no_partial: bool,
}

#[cfg(target_os = "linux")]
pub(crate) struct Journal {
    socket: std::os::unix::net::UnixDatagram,
    metadata: Vec<u8>,
    partial: [Vec<u8>; 2],
    no_partial: bool,
}

#[cfg(target_os = "linux")]
impl Journal {
    pub(crate) fn open(options: Metadata<'_>) -> io::Result<Self> {
        if options.id.is_empty() || options.id.len() > 128 || !options.id.is_ascii() {
            return Err(io::Error::other(
                "journal log ID must be 1..128 ASCII bytes",
            ));
        }
        let mut metadata = Vec::new();
        field(&mut metadata, "CONTAINER_ID_FULL", options.id.as_bytes())?;
        let short = &options.id[..options.id.len().min(12)];
        field(&mut metadata, "CONTAINER_ID", short.as_bytes())?;
        field(
            &mut metadata,
            "SYSLOG_IDENTIFIER",
            options.tag.or(options.name).unwrap_or(short).as_bytes(),
        )?;
        if let Some(name) = options.name {
            field(&mut metadata, "CONTAINER_NAME", name.as_bytes())?;
        }
        if let Some(tag) = options.tag {
            field(&mut metadata, "CONTAINER_TAG", tag.as_bytes())?;
        }
        if options.labels.len() > 32 {
            return Err(io::Error::other("at most 32 log labels are supported"));
        }
        for label in options.labels {
            let (key, value) = label
                .split_once('=')
                .ok_or_else(|| io::Error::other("log label must be NAME=VALUE"))?;
            if key.is_empty()
                || key.len() > 64
                || !key.as_bytes()[0].is_ascii_uppercase()
                || !key
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
                || matches!(key, "MESSAGE" | "PRIORITY" | "SYSLOG_IDENTIFIER")
                || key.starts_with("CONTAINER_")
            {
                return Err(io::Error::other(
                    "log label must be an unreserved uppercase journal field",
                ));
            }
            field(&mut metadata, key, value.as_bytes())?;
        }
        if metadata.len() > 32 * 1024 {
            return Err(io::Error::other("journal metadata exceeds 32KiB bound"));
        }
        let socket = std::os::unix::net::UnixDatagram::unbound()?;
        socket.connect("/run/systemd/journal/socket")?;
        Ok(Self {
            socket,
            metadata,
            partial: [Vec::new(), Vec::new()],
            no_partial: options.no_partial,
        })
    }
    pub(crate) fn write_chunk(&mut self, stream: Stream, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() > 8192 {
            return Err(io::Error::other("journal capture chunk exceeds bound"));
        }
        let index = index(stream);
        for line in bytes.split_inclusive(|b| *b == b'\n') {
            let partial = line.last() != Some(&b'\n');
            if partial && self.partial[index].len() + line.len() < 8192 {
                self.partial[index].extend_from_slice(line);
                continue;
            }
            let mut priority = if matches!(stream, Stream::Stderr) {
                b'3'
            } else {
                b'6'
            };
            let mut line = line;
            if self.partial[index].is_empty()
                && line.len() >= 3
                && line[0] == b'<'
                && (b'0'..=b'7').contains(&line[1])
                && line[2] == b'>'
            {
                priority = line[1];
                line = &line[3..];
            }
            let mut message = std::mem::take(&mut self.partial[index]);
            message.extend_from_slice(line);
            self.send(&message, priority, partial)?;
        }
        Ok(())
    }
    pub(crate) fn end(&mut self, stream: Stream) -> io::Result<()> {
        let bytes = std::mem::take(&mut self.partial[index(stream)]);
        if !bytes.is_empty() {
            self.send(
                &bytes,
                if matches!(stream, Stream::Stderr) {
                    b'3'
                } else {
                    b'6'
                },
                true,
            )?;
        }
        Ok(())
    }
    fn send(&self, message: &[u8], priority: u8, partial: bool) -> io::Result<()> {
        let mut packet = self.metadata.clone();
        field(&mut packet, "MESSAGE", message)?;
        field(&mut packet, "PRIORITY", &[priority])?;
        if partial && !self.no_partial {
            field(&mut packet, "CONTAINER_PARTIAL_MESSAGE", b"true")?;
        }
        loop {
            match self.socket.send(&packet) {
                Ok(count) if count == packet.len() => return Ok(()),
                Ok(_) => return Err(io::Error::other("short journal datagram")),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) if error.raw_os_error() == Some(nix::libc::EMSGSIZE) => {
                    return self.send_memfd(&packet);
                }
                Err(error) => return Err(error),
            }
        }
    }
    fn send_memfd(&self, packet: &[u8]) -> io::Result<()> {
        use nix::{
            fcntl::{FcntlArg, SealFlag, fcntl},
            sys::{
                memfd::{MFdFlags, memfd_create},
                socket::{ControlMessage, MsgFlags, UnixAddr, sendmsg},
            },
        };
        use std::{io::Write, os::fd::AsRawFd};
        let descriptor = memfd_create(
            c"madrillo-journal",
            MFdFlags::MFD_CLOEXEC | MFdFlags::MFD_ALLOW_SEALING,
        )?;
        let mut file = std::fs::File::from(descriptor);
        file.write_all(packet)?;
        fcntl(
            &file,
            FcntlArg::F_ADD_SEALS(
                SealFlag::F_SEAL_SEAL
                    | SealFlag::F_SEAL_SHRINK
                    | SealFlag::F_SEAL_GROW
                    | SealFlag::F_SEAL_WRITE,
            ),
        )?;
        sendmsg::<UnixAddr>(
            self.socket.as_raw_fd(),
            &[],
            &[ControlMessage::ScmRights(&[file.as_raw_fd()])],
            MsgFlags::MSG_NOSIGNAL,
            None,
        )?;
        Ok(())
    }
}
#[cfg(target_os = "linux")]
fn index(stream: Stream) -> usize {
    if matches!(stream, Stream::Stdout) {
        0
    } else {
        1
    }
}

#[cfg(target_os = "linux")]
fn field(packet: &mut Vec<u8>, name: &str, value: &[u8]) -> io::Result<()> {
    let extra = name.len().saturating_add(value.len()).saturating_add(10);
    if packet.len().saturating_add(extra) > 64 * 1024 {
        return Err(io::Error::other("journal entry exceeds 64KiB bound"));
    }
    packet
        .try_reserve(extra)
        .map_err(|_| io::Error::other("journal allocation failed"))?;
    packet.extend_from_slice(name.as_bytes());
    if value.contains(&b'\n') {
        packet.push(b'\n');
        packet.extend_from_slice(&(value.len() as u64).to_le_bytes());
    } else {
        packet.push(b'=');
    }
    packet.extend_from_slice(value);
    packet.push(b'\n');
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub(crate) struct Journal;
#[cfg(not(target_os = "linux"))]
impl Journal {
    pub(crate) fn open(options: Metadata<'_>) -> io::Result<Self> {
        let Metadata {
            id,
            name,
            tag,
            labels,
            no_partial,
        } = options;
        let _ = (id, name, tag, labels, no_partial);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "journald logging requires Linux",
        ))
    }
    pub(crate) fn write_chunk(&mut self, _stream: Stream, _bytes: &[u8]) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }
    pub(crate) fn end(&mut self, _stream: Stream) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }
}
