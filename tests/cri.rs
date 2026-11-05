#![forbid(unsafe_code)]
#![cfg(feature = "cli")]

#[path = "../src/logging/cri.rs"]
mod cri;

use chrono::{DateTime, FixedOffset};
use cri::{Cri, Stream};
use std::io::{self, Write};

fn timestamp() -> Result<DateTime<FixedOffset>, Box<dyn std::error::Error>> {
    Ok(DateTime::parse_from_rfc3339(
        "2026-11-05T12:00:00.123456789+00:00",
    )?)
}

#[test]
fn source_based_full_partial_and_binary_records() -> Result<(), Box<dyn std::error::Error>> {
    let mut output = Vec::new();
    let mut logger = Cri::new(&mut output);
    logger.write_chunk(timestamp()?, Stream::Stdout, b"first\n\npartial\0\xff")?;
    logger.write_chunk(timestamp()?, Stream::Stderr, b"diagnostic\n")?;
    let mut expected = b"2026-11-05T12:00:00.123456789+00:00 stdout F first\n2026-11-05T12:00:00.123456789+00:00 stdout F \n2026-11-05T12:00:00.123456789+00:00 stdout P partial\0\xff\n".to_vec();
    expected.extend_from_slice(b"2026-11-05T12:00:00.123456789+00:00 stderr F diagnostic\n");
    assert_eq!(output, expected);
    Ok(())
}

#[test]
fn partial_chunks_retain_the_original_payload() -> Result<(), Box<dyn std::error::Error>> {
    let mut output = Vec::new();
    let mut logger = Cri::new(&mut output);
    logger.write_chunk(timestamp()?, Stream::Stdout, b"one")?;
    logger.write_chunk(timestamp()?, Stream::Stdout, b" two\n")?;
    assert_eq!(
        output,
        b"2026-11-05T12:00:00.123456789+00:00 stdout P one\n2026-11-05T12:00:00.123456789+00:00 stdout F  two\n"
    );
    Ok(())
}

#[test]
fn empty_capture_does_not_create_a_record() -> Result<(), Box<dyn std::error::Error>> {
    let mut output = Vec::new();
    Cri::new(&mut output).write_chunk(timestamp()?, Stream::Stdout, b"")?;
    assert!(output.is_empty());
    Ok(())
}

#[test]
fn short_writes_are_retried_but_storage_errors_are_returned()
-> Result<(), Box<dyn std::error::Error>> {
    struct Sink {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.bytes.len() >= self.limit {
                return Err(io::Error::other("injected storage failure"));
            }
            let count = bytes.len().min(3).min(self.limit - self.bytes.len());
            self.bytes.extend_from_slice(&bytes[..count]);
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut sink = Sink {
        bytes: Vec::new(),
        limit: 1000,
    };
    Cri::new(&mut sink).write_chunk(timestamp()?, Stream::Stderr, b"complete\n")?;
    assert_eq!(
        sink.bytes,
        b"2026-11-05T12:00:00.123456789+00:00 stderr F complete\n"
    );
    let mut failed = Sink {
        bytes: Vec::new(),
        limit: 12,
    };
    assert!(
        Cri::new(&mut failed)
            .write_chunk(timestamp()?, Stream::Stdout, b"payload\n")
            .is_err()
    );
    assert_eq!(failed.bytes.len(), 12);
    Ok(())
}
