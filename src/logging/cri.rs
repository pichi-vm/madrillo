use chrono::{DateTime, FixedOffset, SecondsFormat};
use std::io::{self, Write};

#[derive(Debug, Clone, Copy)]
pub(crate) enum Stream {
    Stdout,
    Stderr,
}

impl Stream {
    fn name(self) -> &'static [u8] {
        match self {
            Self::Stdout => b"stdout",
            Self::Stderr => b"stderr",
        }
    }
}

/// Encodes conmon's file-log records, with one timestamp per captured chunk.
/// Retention, path ownership, and synchronization belong to the file sink.
pub(crate) struct Cri<W> {
    sink: W,
}

impl<W: Write> Cri<W> {
    pub(crate) fn new(sink: W) -> Self {
        Self { sink }
    }

    pub(crate) fn write_chunk(
        &mut self,
        timestamp: DateTime<FixedOffset>,
        stream: Stream,
        bytes: &[u8],
    ) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let timestamp = timestamp.to_rfc3339_opts(SecondsFormat::Nanos, false);
        for line in bytes.split_inclusive(|byte| *byte == b'\n') {
            let complete = line.last() == Some(&b'\n');
            self.sink.write_all(timestamp.as_bytes())?;
            self.sink.write_all(b" ")?;
            self.sink.write_all(stream.name())?;
            self.sink
                .write_all(if complete { b" F " } else { b" P " })?;
            self.sink.write_all(line)?;
            if !complete {
                self.sink.write_all(b"\n")?;
            }
        }
        Ok(())
    }
}
