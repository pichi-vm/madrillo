use crate::framing::{FrameError, Frames, MAX_FRAME_BYTES};
use crate::input::Input;
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
    collections::VecDeque,
    io::{self, Cursor, Read, Write},
    path::Path,
};

const MAX_CLIENTS: usize = 32;
// JSON byte arrays expand console data by up to four times. Allow the same
// encoded-byte budget as one maximum frame so an ordinary boot report can
// survive a scheduling burst; every client remains independently bounded.
const MAX_PENDING_BYTES: usize = MAX_FRAME_BYTES;

pub(crate) struct Connections {
    listener: Option<Listener>,
    clients: Vec<Client>,
    next_id: u64,
    released: bool,
    extra_streams: Vec<String>,
    /// Input bytes the workload has no room for yet, by the client that sent
    /// them, oldest first: they reach the workload in that order. While a
    /// client has any, its frames are not read: its sender waits, nothing is
    /// lost. They are delivered if the client goes meanwhile. A client holds at
    /// most one frame.
    holding: VecDeque<(u64, Vec<u8>)>,
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
    /// May send `Input`; any number of clients can hold this at once.
    input: bool,
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
            input: false,
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
            // Output waits for attached clients until force or timeout, so
            // only a stalled client after that, or one that never reads its
            // replies, gets here. It is cut off, never the workload/log.
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
        // Drain until actual backpressure. Windows writes are chunked by the
        // transport; one short write must not strand an otherwise writable
        // client's backlog. Pending bytes are already bounded per client.
        while self.written < self.outgoing.len() {
            match self.stream.write(&self.outgoing[self.written..]) {
                Ok(0) => break,
                Ok(count) => self.written += count,
                Err(e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.kind() == io::ErrorKind::Interrupted =>
                {
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        if self.written == self.outgoing.len() {
            self.outgoing.clear();
            self.written = 0;
        }
        Ok(())
    }
}

impl Connections {
    pub(crate) fn close_listener(&mut self) {
        self.listener = None;
    }
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
        client.input = input;
        self.clients.push(client);
        Ok(())
    }
    pub(crate) fn initial_alive(&self) -> bool {
        self.clients
            .iter()
            .any(|client| client.id == 0 && !client.dead)
    }
    pub(crate) fn bind(path: Option<&Path>, extra_streams: Vec<String>) -> io::Result<Self> {
        let listener = if let Some(path) = path {
            #[cfg(unix)]
            let listener = ListenerOptions::new()
                .name(path.to_fs_name::<GenericFilePath>()?)
                .nonblocking(ListenerNonblockingMode::Both)
                .create_sync()?;
            #[cfg(windows)]
            let listener = {
                use interprocess::os::windows::named_pipe::PipeListenerOptions;
                let path = widestring::U16CString::from_os_str(path).map_err(io::Error::other)?;
                PipeListenerOptions::new()
                    .path(path.as_ucstr())
                    .nonblocking(true)
                    // LocalSocket's 512-byte default forces hundreds of polling
                    // turns for one input frame. Bound native buffers explicitly.
                    .input_buffer_size_hint(64 * 1024)
                    .output_buffer_size_hint(64 * 1024)
                    .security_descriptor(Some(crate::security::pipe_descriptor()?))
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
            released: false,
            extra_streams,
            holding: VecDeque::new(),
        })
    }

    /// Keep the part of a client's input the workload has no room for, and
    /// stop reading that client until it is delivered.
    pub(crate) fn hold_input(&mut self, id: u64, bytes: &[u8]) {
        self.holding.push_back((id, bytes.to_vec()));
    }
    /// Hand held input to the workload as it has room, the oldest first and
    /// none past one the workload could not take whole. A client that has gone
    /// still has what it held delivered; it is only not told of a failure.
    pub(crate) fn feed_held(&mut self, input: &mut Input) -> Result<(), FrameError> {
        while let Some((id, bytes)) = self.holding.front_mut() {
            let id = *id;
            match input.accept(bytes) {
                Ok(count) => {
                    bytes.drain(..count);
                    if !bytes.is_empty() {
                        return Ok(());
                    }
                }
                Err(error) => {
                    self.holding.pop_front();
                    self.error(id, ErrorCode::InputClosed, &error.to_string())?;
                    continue;
                }
            }
            self.holding.pop_front();
        }
        Ok(())
    }

    pub(crate) fn poll(&mut self) -> io::Result<Vec<(u64, ClientMessage)>> {
        self.clients.retain(|client| !client.dead);
        if let Some(listener) = &self.listener {
            // Bound accepts per iteration so a flood cannot starve deadlines.
            for _ in 0..4 {
                match accept(listener) {
                    // Filesystem socket permissions are not an access check on
                    // every Unix host. Check the kernel's peer identity before
                    // parsing requests or sending any workload output.
                    #[cfg(unix)]
                    Ok(stream)
                        if stream.peer_creds().ok().and_then(|peer| peer.euid())
                            != Some(rustix::process::geteuid().as_raw()) => {}
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
            // A client whose input is held waits for the workload to read it.
            if self.holding.iter().any(|(id, _)| *id == client.id) {
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
        if streams.len() > 2 + self.extra_streams.len()
            || streams
                .iter()
                .any(|s| s != "stdout" && s != "stderr" && !self.extra_streams.contains(s))
        {
            return self.error(id, ErrorCode::InvalidRequest, "unknown stream");
        }
        if let Some(client) = self.clients.iter_mut().find(|c| c.id == id) {
            client.attached = Some(streams);
            client.input = input;
        }
        self.send(id, ServerMessage::Attached { input })
    }
    pub(crate) fn may_send_input(&self, id: u64) -> bool {
        self.clients.iter().any(|c| c.id == id && c.input)
    }
    pub(crate) fn release_input(&mut self, id: u64) {
        if let Some(client) = self.clients.iter_mut().find(|c| c.id == id) {
            client.input = false;
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
    // Force and timeout stop waiting for slow clients, so the log drains.
    pub(crate) fn release(&mut self) {
        self.released = true;
    }
    pub(crate) fn output_ready(&mut self) -> bool {
        self.released
            || self
                .clients
                .iter_mut()
                .filter(|client| client.attached.is_some() && !client.dead)
                .all(|client| {
                    if client.flush().is_err() {
                        client.dead = true;
                    }
                    client.dead || client.written == client.outgoing.len()
                })
    }
    pub(crate) fn pending(&self) -> bool {
        self.clients
            .iter()
            .any(|c| !c.dead && c.written < c.outgoing.len())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Connections bound to an endpoint of its own, with `count` clients that
    /// have connected to it and been accepted, one at a time (ids 1 to
    /// `count`). The clients are returned to be kept connected, and the
    /// directory to be kept.
    fn connected(
        count: usize,
    ) -> Result<(tempfile::TempDir, Connections, Vec<Stream>), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let endpoint = root.path().join("control.sock");
        let mut connections = Connections::bind(Some(&endpoint), Vec::new())?;
        let mut peers = Vec::new();
        for _ in 0..count {
            peers.push(Stream::connect(
                endpoint.as_path().to_fs_name::<GenericFilePath>()?,
            )?);
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while connections.clients.len() < count {
            assert!(Instant::now() < deadline, "a client was not accepted");
            connections.poll()?;
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok((root, connections, peers))
    }

    /// A workload that reads what it is given, through `cat`, and the input
    /// that feeds it.
    fn reading_workload() -> Result<(Input, Output), Box<dyn std::error::Error>> {
        let (input, stdin) = Input::pipe()?;
        let mut cat = std::process::Command::new("cat")
            .stdin(stdin)
            .stdout(std::process::Stdio::piped())
            .spawn()?;
        let mut stdout = cat.stdout.take().ok_or("no stdout")?;
        let reader = std::thread::spawn(move || {
            let mut received = Vec::new();
            stdout.read_to_end(&mut received).map(|_| received)
        });
        Ok((input, Output { cat, reader }))
    }

    /// What the workload received, and the workload.
    struct Output {
        cat: std::process::Child,
        reader: std::thread::JoinHandle<io::Result<Vec<u8>>>,
    }

    /// Feed the workload until nothing is held, close its input and return
    /// what it received.
    fn finish(
        connections: &mut Connections,
        mut input: Input,
        mut workload: Output,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !input.is_closed() {
            connections.feed_held(&mut input)?;
            if connections.holding.is_empty() {
                input.close_after_flush();
            }
            input.flush()?;
            assert!(Instant::now() < deadline, "held input was not delivered");
            std::thread::sleep(Duration::from_millis(2));
        }
        workload.cat.wait()?;
        Ok(workload.reader.join().map_err(|_| "reader panicked")??)
    }

    /// Input held for several clients reaches the workload in the order it
    /// was held, however many turns that takes and whatever the clients' order.
    #[test]
    fn held_input_of_several_clients_is_delivered_oldest_first()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_root, mut connections, _peers) = connected(2)?;
        let (input, workload) = reading_workload()?;
        // More than the workload's queue holds, from the later-listed client.
        let big = vec![b'a'; 200 * 1024];
        connections.hold_input(2, &big);
        connections.hold_input(1, &[b'b'; 10]);
        let received = finish(&mut connections, input, workload)?;
        assert_eq!(received.len(), big.len() + 10);
        assert!(received[..big.len()].iter().all(|byte| *byte == b'a'));
        assert!(received[big.len()..].iter().all(|byte| *byte == b'b'));
        Ok(())
    }

    /// Input held for a client that is then cut off is still delivered.
    #[test]
    fn input_held_for_a_client_that_goes_is_still_delivered()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_root, mut connections, _peers) = connected(1)?;
        let (input, workload) = reading_workload()?;
        connections.hold_input(1, b"held");
        connections.clients[0].dead = true;
        connections.poll()?;
        assert!(connections.clients.is_empty());
        assert_eq!(finish(&mut connections, input, workload)?, b"held");
        Ok(())
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
