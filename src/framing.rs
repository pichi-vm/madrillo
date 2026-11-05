use madrillo::{PROTOCOL_VERSION, Request, Response};
use serde::{Serialize, de::DeserializeOwned};
use std::io::{self, Read, Write};
use thiserror::Error;

pub(crate) const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Debug, Error)]
pub(crate) enum FrameError {
    #[error("transport: {0}")]
    Io(#[from] io::Error),
    #[error("frame length {0} is outside the permitted range")]
    Length(u32),
    #[error("frame allocation failed")]
    Allocation,
    #[error("invalid frame: {0}")]
    Encoding(#[from] serde_json::Error),
    #[error("unsupported protocol version {0}")]
    Version(u16),
}

#[derive(Debug)]
pub(crate) struct Frames<T> {
    io: T,
}

impl<T> Frames<T> {
    pub(crate) fn new(io: T) -> Self {
        Self { io }
    }
}

impl<T: Read> Frames<T> {
    pub(crate) fn request(&mut self) -> Result<Option<Request>, FrameError> {
        let Some(request) = self.decode::<Request>()? else {
            return Ok(None);
        };
        if request.version != PROTOCOL_VERSION {
            return Err(FrameError::Version(request.version));
        }
        Ok(Some(request))
    }

    fn decode<M: DeserializeOwned>(&mut self) -> Result<Option<M>, FrameError> {
        let mut header = [0_u8; 4];
        loop {
            match self.io.read(&mut header[..1]) {
                Ok(0) => return Ok(None),
                Ok(_) => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            }
        }
        self.io.read_exact(&mut header[1..])?;
        let size = u32::from_be_bytes(header);
        let length = usize::try_from(size).map_err(|_| FrameError::Length(size))?;
        if length == 0 || length > MAX_FRAME_BYTES {
            return Err(FrameError::Length(size));
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|_| FrameError::Allocation)?;
        bytes.resize(length, 0);
        self.io.read_exact(&mut bytes)?;
        Ok(Some(serde_json::from_slice(&bytes)?))
    }
}

impl<T: Write> Frames<T> {
    pub(crate) fn response(&mut self, message: &Response) -> Result<(), FrameError> {
        self.encode(message)
    }

    fn encode<M: Serialize>(&mut self, message: &M) -> Result<(), FrameError> {
        let mut buffer = BoundedBuffer { bytes: Vec::new() };
        serde_json::to_writer(&mut buffer, message)?;
        let length = u32::try_from(buffer.bytes.len()).map_err(|_| FrameError::Length(u32::MAX))?;
        self.io.write_all(&length.to_be_bytes())?;
        self.io.write_all(&buffer.bytes)?;
        self.io.flush()?;
        Ok(())
    }
}

struct BoundedBuffer {
    bytes: Vec<u8>,
}

impl Write for BoundedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let remaining = MAX_FRAME_BYTES.saturating_sub(self.bytes.len());
        if bytes.len() > remaining {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "encoded frame exceeds the limit",
            ));
        }
        self.bytes
            .try_reserve(bytes.len())
            .map_err(|_| io::Error::other("frame allocation failed"))?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
