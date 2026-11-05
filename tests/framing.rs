#![forbid(unsafe_code)]
#![cfg(feature = "cli")]

#[path = "../src/framing.rs"]
mod framing;

use framing::{FrameError, Frames, MAX_FRAME_BYTES};
use madrillo::{ClientMessage, PROTOCOL_VERSION, Request, Response, ServerMessage};
use std::io::{self, Cursor, Read};

fn packet(json: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut bytes = u32::try_from(json.len())?.to_be_bytes().to_vec();
    bytes.extend_from_slice(json);
    Ok(bytes)
}

#[test]
fn empty_stream_is_disconnect_but_partial_header_is_error() {
    assert!(matches!(Frames::new(Cursor::new([])).request(), Ok(None)));
    for length in 1..4 {
        let mut frames = Frames::new(Cursor::new(vec![0; length]));
        assert!(
            matches!(frames.request(), Err(FrameError::Io(error)) if error.kind() == io::ErrorKind::UnexpectedEof)
        );
    }
}

#[test]
fn oversized_prefix_is_rejected_without_reading_the_body() -> Result<(), Box<dyn std::error::Error>>
{
    struct Counted<'a> {
        reader: Cursor<Vec<u8>>,
        read: &'a mut usize,
    }
    impl Read for Counted<'_> {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            let count = self.reader.read(bytes)?;
            *self.read += count;
            Ok(count)
        }
    }
    for size in [0_u32, u32::try_from(MAX_FRAME_BYTES)? + 1, u32::MAX] {
        let mut read = 0;
        let mut frames = Frames::new(Counted {
            reader: Cursor::new(size.to_be_bytes().to_vec()),
            read: &mut read,
        });
        assert!(matches!(frames.request(), Err(FrameError::Length(actual)) if actual == size));
        assert_eq!(read, 4);
    }
    Ok(())
}

#[test]
fn partial_payload_is_not_a_clean_disconnect() -> Result<(), Box<dyn std::error::Error>> {
    let mut bytes = packet(b"{}")?;
    bytes.pop();
    let mut frames = Frames::new(Cursor::new(bytes));
    assert!(
        matches!(frames.request(), Err(FrameError::Io(error)) if error.kind() == io::ErrorKind::UnexpectedEof)
    );
    Ok(())
}

#[test]
fn binary_input_survives_multiple_frames() -> Result<(), Box<dyn std::error::Error>> {
    let message = Request {
        version: PROTOCOL_VERSION,
        message: ClientMessage::Input {
            bytes: vec![0, 3, 4, 255],
        },
    };
    let encoded = packet(&serde_json::to_vec(&message)?)?;
    let mut bytes = encoded.clone();
    bytes.extend(encoded);
    let mut frames = Frames::new(Cursor::new(bytes));
    assert_eq!(frames.request()?, Some(message.clone()));
    assert_eq!(frames.request()?, Some(message));
    assert_eq!(frames.request()?, None);
    Ok(())
}

#[test]
fn version_and_unknown_messages_are_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let wrong = packet(br#"{"version":65535,"message":{"type":"wait"}}"#)?;
    assert!(matches!(
        Frames::new(Cursor::new(wrong)).request(),
        Err(FrameError::Version(65535))
    ));
    for json in [
        br#"{"version":1,"message":{"type":"invented"}}"#.as_slice(),
        br#"{"version":1,"message":{"type":"input","bytes":[256]}}"#.as_slice(),
        br#"{"version":1,"message":{"type":"input","bytes":[],"extra":true}}"#.as_slice(),
        br#"{"version":1,"message":{"type":"wait"},"extra":true}"#.as_slice(),
    ] {
        assert!(matches!(
            Frames::new(Cursor::new(packet(json)?)).request(),
            Err(FrameError::Encoding(_))
        ));
    }
    Ok(())
}

#[test]
fn response_is_framed_and_oversize_never_emits_a_prefix() -> Result<(), Box<dyn std::error::Error>>
{
    let response = Response {
        version: PROTOCOL_VERSION,
        message: ServerMessage::Output {
            stream: "stdout".into(),
            bytes: vec![0, 255],
        },
    };
    let mut bytes = Vec::new();
    Frames::new(&mut bytes).response(&response)?;
    let mut cursor = Cursor::new(bytes);
    let mut header = [0_u8; 4];
    cursor.read_exact(&mut header)?;
    let mut json = Vec::new();
    cursor.read_to_end(&mut json)?;
    assert_eq!(usize::try_from(u32::from_be_bytes(header))?, json.len());
    assert_eq!(serde_json::from_slice::<Response>(&json)?, response);

    let oversized = Response {
        version: PROTOCOL_VERSION,
        message: ServerMessage::Output {
            stream: "stdout".into(),
            bytes: vec![255; MAX_FRAME_BYTES],
        },
    };
    let mut destination = Vec::new();
    assert!(Frames::new(&mut destination).response(&oversized).is_err());
    assert!(destination.is_empty());
    Ok(())
}
