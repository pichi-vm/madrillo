//! An ordinary child exchanges exact bytes through inherited named-port pipes.
//! Only this qualification fixture understands its arguments; the monitor does not.
use filedescriptor::{
    AsRawFileDescriptor, FileDescriptor, IntoRawFileDescriptor, RawFileDescriptor,
};
use std::{
    io::{Read, Write},
    sync::mpsc,
    time::Duration,
};

#[derive(serde::Deserialize)]
struct Exchange {
    request: Vec<u8>,
    response: Vec<u8>,
    marker: std::path::PathBuf,
}

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

fn pipe(spec: &str, key: &str) -> Result<FileDescriptor, Box<dyn std::error::Error>> {
    let value = spec
        .split([',', ']'])
        .find_map(|field| field.strip_prefix(key))
        .ok_or("missing inherited port pipe")?
        .parse::<usize>()?;
    #[cfg(unix)]
    let raw = i32::try_from(value)?;
    #[cfg(windows)]
    let raw = value as RawFileDescriptor;
    Ok(FileDescriptor::new(Inherited(raw)))
}

pub(crate) fn run() -> Result<i32, Box<dyn std::error::Error>> {
    let exchange: Exchange =
        serde_json::from_str(&std::env::var("MADRILLO_FIXTURE_PORT_EXCHANGE")?)?;
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let spec = arguments
        .windows(2)
        .find(|pair| pair[0] == "--console")
        .ok_or("named console arguments missing")?[1]
        .clone();
    let mut reader = pipe(&spec, "rx=pipe:")?;
    let mut writer = pipe(&spec, "tx=pipe:")?;
    let expected = exchange.response.len();
    if expected == 0 || expected > 65536 {
        return Err("invalid response size".into());
    }
    let (first_send, first) = mpsc::channel();
    let (done_send, done) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| -> std::io::Result<Vec<u8>> {
            let mut bytes = vec![0; expected];
            reader.read_exact(&mut bytes[..1])?;
            let _ = first_send.send(());
            reader.read_exact(&mut bytes[1..])?;
            Ok(bytes)
        })();
        let _ = done_send.send(result);
    });
    if !matches!(
        first.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ) {
        return Err("host sent data or closed the port before the child's request".into());
    }
    writer.write_all(&exchange.request)?;
    drop(writer);
    if done.recv_timeout(Duration::from_secs(5))?? != exchange.response {
        return Err("host response changed across inherited pipes".into());
    }
    std::fs::write(exchange.marker, serde_json::to_vec(&arguments)?)?;
    // A nonzero child exit lets the caller test the channel without fabricating
    // a successful guest build or creating a VM image.
    Ok(37)
}
