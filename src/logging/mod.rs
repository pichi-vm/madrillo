pub(crate) mod cri;
pub(crate) mod file;
use chrono::Local;
use cri::Stream;
use file::FileLog;
use std::{
    io::{self, Read},
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
};

const CHUNK_BYTES: usize = 8192;
const QUEUED_CHUNKS: usize = 16;
const MAX_ERRORS: usize = 32;
enum Message {
    Data(Stream, Vec<u8>),
    End(Option<String>),
}
#[derive(Default)]
pub(crate) struct Report {
    pub(crate) errors: Vec<String>,
}
pub(crate) struct Capture {
    sender: SyncSender<Message>,
    pub(crate) done: Receiver<Report>,
    pub(crate) output: Receiver<(Stream, Vec<u8>)>,
}

impl Capture {
    pub(crate) fn start(mut log: Option<FileLog>, sync: bool, sources: usize) -> io::Result<Self> {
        let (sender, incoming) = mpsc::sync_channel(QUEUED_CHUNKS);
        let (done, report) = mpsc::sync_channel(1);
        let (output, live) = mpsc::sync_channel(QUEUED_CHUNKS);
        thread::Builder::new()
            .name("madrillo-log".into())
            .spawn(move || {
                let mut report = Report::default();
                let mut ended = 0;
                while ended < sources {
                    let error = match incoming.recv() {
                        Ok(Message::Data(stream, bytes)) => {
                            let error = log
                                .as_mut()
                                .and_then(|sink| {
                                    sink.write_chunk(Local::now().fixed_offset(), stream, &bytes)
                                        .err()
                                })
                                .map(|e| e.to_string());
                            if output.send((stream, bytes)).is_err() {
                                break;
                            }
                            error
                        }
                        Ok(Message::End(error)) => {
                            ended += 1;
                            error
                        }
                        Err(_) => {
                            report.errors.push("capture workers disconnected".into());
                            break;
                        }
                    };
                    if let Some(error) = error
                        && report.errors.len() < MAX_ERRORS
                    {
                        report.errors.push(error);
                    }
                }
                if sync
                    && let Some(log) = log
                    && let Err(error) = log.sync()
                {
                    report.errors.push(format!("log synchronization: {error}"));
                }
                let _ = done.send(report);
            })?;
        Ok(Self {
            sender,
            done: report,
            output: live,
        })
    }
    pub(crate) fn read(
        &self,
        mut reader: impl Read + Send + 'static,
        stream: Stream,
    ) -> io::Result<()> {
        let sender = self.sender.clone();
        thread::Builder::new()
            .name("madrillo-capture".into())
            .spawn(move || {
                let mut buffer = [0; CHUNK_BYTES];
                let error = loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break None,
                        Ok(count) => {
                            if sender
                                .send(Message::Data(stream, buffer[..count].to_vec()))
                                .is_err()
                            {
                                return;
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                        Err(error) => break Some(format!("capture read: {error}")),
                    }
                };
                let _ = sender.send(Message::End(error));
            })?;
        Ok(())
    }
}
