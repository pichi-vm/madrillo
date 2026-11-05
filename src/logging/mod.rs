pub(crate) mod cri;
pub(crate) mod file;
pub(crate) mod journal;
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
    End(Stream, Option<String>),
    Reopen(u64),
}
#[derive(Default)]
pub(crate) struct Report {
    pub(crate) errors: Vec<String>,
}
pub(crate) struct Capture {
    sender: SyncSender<Message>,
    pub(crate) done: Receiver<Report>,
    pub(crate) output: Receiver<(Stream, Vec<u8>)>,
    pub(crate) controls: Receiver<(u64, Result<(), String>)>,
}

impl Capture {
    pub(crate) fn start(
        mut log: Option<FileLog>,
        mut journal: Option<journal::Journal>,
        sync: bool,
        sources: usize,
    ) -> io::Result<Self> {
        let (sender, incoming) = mpsc::sync_channel(QUEUED_CHUNKS);
        let (done, report) = mpsc::sync_channel(1);
        let (output, live) = mpsc::sync_channel(QUEUED_CHUNKS);
        let (controls, responses) = mpsc::sync_channel(32);
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
                            if let Some(journal) = &mut journal
                                && let Err(error) = journal.write_chunk(stream, &bytes)
                                && report.errors.len() < MAX_ERRORS
                            {
                                report.errors.push(format!("journal write: {error}"));
                            }
                            if output.send((stream, bytes)).is_err() {
                                break;
                            }
                            error
                        }
                        Ok(Message::Reopen(id)) => {
                            let result = log
                                .as_mut()
                                .ok_or_else(|| "no file log configured".to_owned())
                                .and_then(|log| log.reopen().map_err(|e| e.to_string()));
                            let error = result.as_ref().err().cloned();
                            // The supervisor drains these independently of client I/O.
                            let _ = controls.send((id, result));
                            error
                        }
                        Ok(Message::End(stream, error)) => {
                            if let Some(journal) = &mut journal
                                && let Err(error) = journal.end(stream)
                                && report.errors.len() < MAX_ERRORS
                            {
                                report
                                    .errors
                                    .push(format!("journal final partial: {error}"));
                            }
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
            controls: responses,
        })
    }
    pub(crate) fn reopen(&self, id: u64) -> io::Result<()> {
        self.sender
            .try_send(Message::Reopen(id))
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => {
                    io::Error::new(io::ErrorKind::WouldBlock, "log worker queue is full")
                }
                mpsc::TrySendError::Disconnected(_) => {
                    io::Error::new(io::ErrorKind::BrokenPipe, "log worker has stopped")
                }
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
                let _ = sender.send(Message::End(stream, error));
            })?;
        Ok(())
    }
}
