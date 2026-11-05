pub(crate) mod cri;
pub(crate) mod extra;
pub(crate) mod file;
pub(crate) mod journal;
use chrono::Local;
use cri::Stream;
use file::FileLog;
use std::{
    io::{self, Read},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::Duration,
};

const CHUNK_BYTES: usize = 8192;
const QUEUED_CHUNKS: usize = 16;
const MAX_ERRORS: usize = 32;
#[derive(Clone, Copy)]
enum Source {
    Console(Stream),
    Extra(usize),
}
enum Message {
    Data(Source, Vec<u8>),
    End(Source, Option<String>),
    Reopen(u64),
}
#[derive(Default)]
pub(crate) struct Report {
    pub(crate) errors: Vec<String>,
}
pub(crate) struct Capture {
    sender: SyncSender<Message>,
    pub(crate) done: Receiver<Report>,
    pub(crate) output: Receiver<(String, Vec<u8>)>,
    pub(crate) controls: Receiver<(u64, Result<(), String>)>,
    stop_extra: Arc<AtomicBool>,
    extra_workers: Vec<thread::JoinHandle<()>>,
}

impl Capture {
    pub(crate) fn start(
        mut log: Option<FileLog>,
        mut journal: Option<journal::Journal>,
        sync: bool,
        sources: usize,
        mut extra: Vec<(String, FileLog)>,
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
                        Ok(Message::Data(source, bytes)) => {
                            let (name, error) = match source {
                                Source::Console(stream) => {
                                    let error = log
                                        .as_mut()
                                        .and_then(|sink| {
                                            sink.write_chunk(
                                                Local::now().fixed_offset(),
                                                stream,
                                                &bytes,
                                            )
                                            .err()
                                        })
                                        .map(|e| e.to_string());
                                    if let Some(journal) = &mut journal
                                        && let Err(error) = journal.write_chunk(stream, &bytes)
                                        && report.errors.len() < MAX_ERRORS
                                    {
                                        report.errors.push(format!("journal write: {error}"));
                                    }
                                    (stream.name().to_owned(), error)
                                }
                                Source::Extra(index) => {
                                    let Some((name, sink)) = extra.get_mut(index) else {
                                        report.errors.push("unknown capture source".into());
                                        break;
                                    };
                                    let error = sink
                                        .write_raw(&bytes)
                                        .err()
                                        .map(|e| format!("{name}: {e}"));
                                    (name.clone(), error)
                                }
                            };
                            if output.send((name, bytes)).is_err() {
                                break;
                            }
                            error
                        }
                        Ok(Message::Reopen(id)) => {
                            let result = (|| {
                                if log.is_none() && extra.is_empty() {
                                    return Err("no file log configured".to_owned());
                                }
                                if let Some(log) = &mut log {
                                    log.reopen().map_err(|e| e.to_string())?;
                                }
                                for (name, sink) in &mut extra {
                                    sink.reopen().map_err(|e| format!("{name}: {e}"))?;
                                }
                                Ok(())
                            })();
                            let error = result.as_ref().err().cloned();
                            // The supervisor drains these independently of client I/O.
                            let _ = controls.send((id, result));
                            error
                        }
                        Ok(Message::End(source, error)) => {
                            if let Source::Console(stream) = source
                                && let Some(journal) = &mut journal
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
                if sync {
                    for (name, sink) in extra {
                        if let Err(error) = sink.sync()
                            && report.errors.len() < MAX_ERRORS
                        {
                            report
                                .errors
                                .push(format!("{name} synchronization: {error}"));
                        }
                    }
                }
                let _ = done.send(report);
            })?;
        Ok(Self {
            sender,
            done: report,
            output: live,
            controls: responses,
            stop_extra: Arc::new(AtomicBool::new(false)),
            extra_workers: Vec::new(),
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
                                .send(Message::Data(
                                    Source::Console(stream),
                                    buffer[..count].to_vec(),
                                ))
                                .is_err()
                            {
                                return;
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                        Err(error) => break Some(format!("capture read: {error}")),
                    }
                };
                let _ = sender.send(Message::End(Source::Console(stream), error));
            })?;
        Ok(())
    }

    pub(crate) fn read_extra(&mut self, mut reader: extra::Reader, index: usize) -> io::Result<()> {
        let sender = self.sender.clone();
        let stop = Arc::clone(&self.stop_extra);
        let worker = thread::Builder::new()
            .name("madrillo-extra-capture".into())
            .spawn(move || {
                // Bounded retry permits cancellation even when a log sink is blocked.
                let send = |mut message| {
                    while !stop.load(Ordering::Acquire) {
                        match sender.try_send(message) {
                            Ok(()) => return true,
                            Err(mpsc::TrySendError::Disconnected(_)) => return false,
                            Err(mpsc::TrySendError::Full(value)) => message = value,
                        }
                        thread::sleep(Duration::from_millis(2));
                    }
                    false
                };
                let mut bytes = [0; CHUNK_BYTES];
                let error = loop {
                    if stop.load(Ordering::Acquire) {
                        return;
                    }
                    match reader.read(&mut bytes) {
                        Ok(0) => break None,
                        Ok(count) => {
                            if !send(Message::Data(Source::Extra(index), bytes[..count].to_vec())) {
                                return;
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2))
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                        Err(error) => break Some(format!("extra capture read: {error}")),
                    }
                };
                // Release the pipe before reporting completion, including when the
                // bounded message queue is full. No retained reader reaches a hook.
                drop(reader);
                send(Message::End(Source::Extra(index), error));
            })?;
        self.extra_workers.push(worker);
        Ok(())
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop_extra.store(true, Ordering::Release);
        for worker in self.extra_workers.drain(..) {
            let _ = worker.join();
        }
    }
}
