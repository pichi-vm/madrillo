use crate::framing::{FrameError, Frames, MAX_FRAME_BYTES};
use crate::transport::Transport;
use interprocess::local_socket::Stream;
#[cfg(unix)]
use interprocess::local_socket::{
    GenericFilePath, Listener, ListenerNonblockingMode, ListenerOptions, prelude::*,
};
#[cfg(windows)]
type Listener = interprocess::os::windows::named_pipe::PipeListener<
    interprocess::os::windows::named_pipe::pipe_mode::Bytes,
    interprocess::os::windows::named_pipe::pipe_mode::Bytes,
>;
use madrillo::{ClientMessage, ErrorCode, PROTOCOL_VERSION, Request, Response, ServerMessage};
use std::{
    io::{self, Cursor, Read, Write},
    path::Path,
};

const MAX_CLIENTS: usize = 32;
const MAX_PENDING_BYTES: usize = 256 * 1024;

pub(crate) struct Connections {
    listener: Option<Listener>,
    clients: Vec<Client>,
    next_id: u64,
    owner: Option<u64>,
}

struct Client {
    id: u64,
    stream: Transport,
    header: [u8; 4],
    header_read: usize,
    payload: Vec<u8>,
    payload_read: usize,
    outgoing: Vec<u8>,
    written: usize,
    attached: Option<Vec<String>>,
    dead: bool,
}

#[cfg(windows)]
impl Drop for Client {
    fn drop(&mut self) {
        let Stream::NamedPipe(stream) = &self.stream.native;
        stream.inner().assume_flushed();
    }
}

impl Client {
    fn new(id: u64, stream: Stream) -> io::Result<Self> {
        Ok(Self {
            id,
            stream: Transport::new(stream)?,
            header: [0; 4],
            header_read: 0,
            payload: Vec::new(),
            payload_read: 0,
            outgoing: Vec::new(),
            written: 0,
            attached: None,
            dead: false,
        })
    }
    fn request(&mut self) -> Result<Option<Request>, FrameError> {
        if self.header_read < 4 {
            match self.stream.read(&mut self.header[self.header_read..]) {
                Ok(0) => {
                    self.dead = true;
                    return Ok(None);
                }
                Ok(count) => self.header_read += count,
                Err(e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.kind() == io::ErrorKind::Interrupted =>
                {
                    return Ok(None);
                }
                Err(e) => return Err(e.into()),
            }
            if self.header_read < 4 {
                return Ok(None);
            }
            let size = u32::from_be_bytes(self.header);
            let length = usize::try_from(size).map_err(|_| FrameError::Length(size))?;
            if length == 0 || length > MAX_FRAME_BYTES {
                return Err(FrameError::Length(size));
            }
            self.payload
                .try_reserve_exact(length)
                .map_err(|_| FrameError::Allocation)?;
            self.payload.resize(length, 0);
        }
        match self.stream.read(&mut self.payload[self.payload_read..]) {
            Ok(0) => {
                self.dead = true;
                return Ok(None);
            }
            Ok(count) => self.payload_read += count,
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::Interrupted =>
            {
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        }
        if self.payload_read < self.payload.len() {
            return Ok(None);
        }
        let frame = Cursor::new(&self.header).chain(Cursor::new(&self.payload));
        let message = Frames::new(frame).request()?;
        self.header_read = 0;
        self.payload_read = 0;
        self.payload.clear();
        Ok(message)
    }

    fn send(&mut self, message: ServerMessage) -> Result<(), FrameError> {
        if self.dead {
            return Ok(());
        }
        if self.flush().is_err() {
            self.dead = true;
            return Ok(());
        }
        let mut frame = Vec::new();
        Frames::new(&mut frame).response(&Response {
            version: PROTOCOL_VERSION,
            message,
        })?;
        if self
            .outgoing
            .len()
            .saturating_sub(self.written)
            .saturating_add(frame.len())
            > MAX_PENDING_BYTES
        {
            // A stalled viewer loses its connection, never the workload/log.
            self.dead = true;
            return Ok(());
        }
        if self.written > 0 {
            self.outgoing.drain(..self.written);
            self.written = 0;
        }
        self.outgoing
            .try_reserve(frame.len())
            .map_err(|_| FrameError::Allocation)?;
        self.outgoing.extend_from_slice(&frame);
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.written == self.outgoing.len() {
            return Ok(());
        }
        match self.stream.write(&self.outgoing[self.written..]) {
            Ok(count) => self.written += count,
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
        if self.written == self.outgoing.len() {
            self.outgoing.clear();
            self.written = 0;
        }
        Ok(())
    }
}

impl Connections {
    pub(crate) fn finish(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        self.clients.clear();
        #[cfg(windows)]
        {
            use std::{
                sync::mpsc,
                thread,
                time::{Duration, Instant},
            };
            let (done, completed) = mpsc::sync_channel(MAX_CLIENTS);
            let mut count = 0;
            for client in self.clients.drain(..).filter(|client| !client.dead) {
                let done = done.clone();
                // At most MAX_CLIENTS final drain workers. Routine disconnects
                // never enter the dependency's implicit pipe-flushing pool.
                thread::Builder::new()
                    .name("madrillo-final-delivery".into())
                    .spawn(move || {
                        let Stream::NamedPipe(stream) = &client.stream.native;
                        let _ = stream.inner().flush();
                        let _ = done.send(());
                    })?;
                count += 1;
            }
            let deadline = Instant::now() + Duration::from_secs(1);
            for _ in 0..count {
                if completed
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .is_err()
                {
                    break;
                }
            }
        }
        Ok(())
    }
    pub(crate) fn initial(&mut self, stream: Stream, input: bool) -> io::Result<()> {
        let mut client = Client::new(0, stream)?;
        client.attached = Some(Vec::new());
        self.clients.push(client);
        if input {
            self.owner = Some(0);
        }
        Ok(())
    }
    pub(crate) fn initial_alive(&self) -> bool {
        self.clients
            .iter()
            .any(|client| client.id == 0 && !client.dead)
    }
    pub(crate) fn bind(path: Option<&Path>) -> io::Result<Self> {
        let listener = if let Some(path) = path {
            #[cfg(unix)]
            let listener = ListenerOptions::new()
                .name(path.to_fs_name::<GenericFilePath>()?)
                .nonblocking(ListenerNonblockingMode::Both)
                .create_sync()?;
            #[cfg(windows)]
            let listener = {
                use interprocess::os::windows::{
                    named_pipe::PipeListenerOptions, security_descriptor::SecurityDescriptor,
                };
                let sddl = widestring::U16CString::from_str("D:P(A;;GA;;;OW)")
                    .map_err(io::Error::other)?;
                let path = widestring::U16CString::from_os_str(path).map_err(io::Error::other)?;
                PipeListenerOptions::new()
                    .path(path.as_ucstr())
                    .nonblocking(true)
                    // LocalSocket's 512-byte default forces hundreds of polling
                    // turns for one input frame. Bound native buffers explicitly.
                    .input_buffer_size_hint(64 * 1024)
                    .output_buffer_size_hint(64 * 1024)
                    .security_descriptor(Some(SecurityDescriptor::deserialize(&sddl)?))
                    .create()?
            };
            Some(listener)
        } else {
            None
        };
        Ok(Self {
            listener,
            clients: Vec::new(),
            next_id: 1,
            owner: None,
        })
    }

    pub(crate) fn poll(&mut self) -> io::Result<Vec<(u64, ClientMessage)>> {
        self.clients.retain(|client| !client.dead);
        if self
            .owner
            .is_some_and(|id| !self.clients.iter().any(|c| c.id == id))
        {
            self.owner = None;
        }
        if let Some(listener) = &self.listener {
            // Bound accepts per iteration so a flood cannot starve deadlines.
            for _ in 0..4 {
                match accept(listener) {
                    Ok(stream) if self.clients.len() < MAX_CLIENTS => {
                        self.clients.push(Client::new(self.next_id, stream)?);
                        self.next_id = self
                            .next_id
                            .checked_add(1)
                            .ok_or_else(|| io::Error::other("connection ID exhausted"))?;
                    }
                    Ok(_) => {}
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) => return Err(error),
                }
            }
        }
        let mut messages = Vec::new();
        for client in &mut self.clients {
            if client.flush().is_err() {
                client.dead = true;
                continue;
            }
            match client.request() {
                Ok(Some(request)) => messages.push((client.id, request.message)),
                Ok(None) => {}
                Err(_) => client.dead = true,
            }
        }
        Ok(messages)
    }

    pub(crate) fn attach(
        &mut self,
        id: u64,
        input: bool,
        streams: Vec<String>,
    ) -> Result<(), FrameError> {
        if streams.len() > 2 || streams.iter().any(|s| s != "stdout" && s != "stderr") {
            return self.error(id, ErrorCode::InvalidRequest, "unknown stream");
        }
        if input && self.owner.is_some_and(|owner| owner != id) {
            return self.error(id, ErrorCode::InputOwned, "another client owns input");
        }
        if input {
            self.owner = Some(id);
        } else if self.owner == Some(id) {
            self.owner = None;
        }
        if let Some(client) = self.clients.iter_mut().find(|c| c.id == id) {
            client.attached = Some(streams);
        }
        self.send(id, ServerMessage::Attached { input })
    }
    pub(crate) fn owns_input(&self, id: u64) -> bool {
        self.owner == Some(id)
    }
    pub(crate) fn release_input(&mut self, id: u64) {
        if self.owner == Some(id) {
            self.owner = None;
        }
    }
    pub(crate) fn send(&mut self, id: u64, message: ServerMessage) -> Result<(), FrameError> {
        if let Some(client) = self.clients.iter_mut().find(|c| c.id == id) {
            client.send(message)?;
        }
        Ok(())
    }
    pub(crate) fn error(
        &mut self,
        id: u64,
        code: ErrorCode,
        message: &str,
    ) -> Result<(), FrameError> {
        self.send(
            id,
            ServerMessage::Error {
                code,
                message: message.into(),
            },
        )
    }
    pub(crate) fn output(&mut self, stream: &str, bytes: Vec<u8>) -> Result<(), FrameError> {
        for client in &mut self.clients {
            if client
                .attached
                .as_ref()
                .is_some_and(|streams| streams.is_empty() || streams.iter().any(|s| s == stream))
            {
                client.send(ServerMessage::Output {
                    stream: stream.into(),
                    bytes: bytes.clone(),
                })?;
            }
        }
        Ok(())
    }
    pub(crate) fn broadcast(&mut self, message: ServerMessage) -> Result<(), FrameError> {
        for client in &mut self.clients {
            client.send(message.clone())?;
        }
        Ok(())
    }
    pub(crate) fn pending(&self) -> bool {
        self.clients
            .iter()
            .any(|c| !c.dead && c.written < c.outgoing.len())
    }
}

fn accept(listener: &Listener) -> io::Result<Stream> {
    #[cfg(unix)]
    {
        listener.accept()
    }
    #[cfg(windows)]
    {
        let stream: interprocess::os::windows::named_pipe::local_socket::Stream =
            listener.accept()?.into();
        Ok(stream.into())
    }
}
