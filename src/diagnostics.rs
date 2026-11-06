//! Monitor diagnostics are separate from workload logs and never block supervision.
use std::{
    fmt,
    io::{self, Write},
    str::FromStr,
    sync::mpsc::{self, SyncSender},
    thread,
    time::Duration,
};

const QUEUE: usize = 32;
const MESSAGE_BYTES: usize = 2048;
const FLUSH_DEADLINE: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Level {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl FromStr for Level {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "error" | "fatal" | "panic" => Ok(Self::Error),
            "warn" | "warning" => Ok(Self::Warn),
            "info" => Ok(Self::Info),
            "debug" => Ok(Self::Debug),
            "trace" => Ok(Self::Trace),
            _ => Err("log level must be error, warn, info, debug, or trace"),
        }
    }
}

enum Message {
    Record(Level, String),
    Flush(SyncSender<Vec<String>>),
}

pub(crate) struct Diagnostics {
    level: Level,
    sender: SyncSender<Message>,
    dropped: usize,
}

impl Diagnostics {
    // Call only after adopting inherited resources: logging may open descriptors.
    pub(crate) fn start(level: Level, syslog: bool) -> io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(QUEUE);
        thread::Builder::new()
            .name("diagnostics".into())
            .spawn(move || {
                let mut errors = Vec::new();
                #[cfg(all(unix, not(target_os = "macos")))]
                let mut logger = if syslog {
                    match syslog::unix(syslog::Formatter3164 {
                        facility: syslog::Facility::LOG_USER,
                        hostname: None,
                        process: "madrillo".into(),
                        pid: std::process::id(),
                    }) {
                        Ok(logger) => Some(logger),
                        Err(error) => {
                            errors.push(format!("diagnostics syslog: {error}"));
                            None
                        }
                    }
                } else {
                    None
                };
                // Local socket writes bypass Apple's unified logging API.
                // Keep its native filtering/delivery policy; submission is not
                // an acknowledgement of durable storage by the system service.
                #[cfg(target_os = "macos")]
                let logger = syslog.then(|| oslog::OsLog::new("org.pichi.madrillo", "monitor"));
                #[cfg(not(unix))]
                let _ = syslog;
                let mut stderr_failed = false;
                while let Ok(message) = receiver.recv() {
                    match message {
                        Message::Record(level, text) => {
                            #[cfg(all(unix, not(target_os = "macos")))]
                            if let Some(sink) = &mut logger {
                                let sent = match level {
                                    Level::Error => sink.err(text.as_str()),
                                    Level::Warn => sink.warning(text.as_str()),
                                    Level::Info => sink.info(text.as_str()),
                                    Level::Debug | Level::Trace => sink.debug(text.as_str()),
                                };
                                if let Err(error) = sent {
                                    errors.push(format!("diagnostics syslog: {error}"));
                                    logger = None;
                                }
                            }
                            #[cfg(target_os = "macos")]
                            if let Some(sink) = &logger {
                                let native_level = match level {
                                    Level::Error => oslog::Level::Error,
                                    Level::Warn => oslog::Level::Default,
                                    Level::Info => oslog::Level::Info,
                                    Level::Debug | Level::Trace => oslog::Level::Debug,
                                };
                                sink.with_level(native_level, &format!("[{level:?}]: {text}"));
                            }
                            if !stderr_failed
                                && let Err(error) =
                                    writeln!(io::stderr().lock(), "madrillo [{level:?}]: {text}")
                            {
                                errors.push(format!("diagnostics stderr: {error}"));
                                stderr_failed = true;
                            }
                        }
                        Message::Flush(reply) => {
                            let _ = reply.send(std::mem::take(&mut errors));
                        }
                    }
                }
            })?;
        Ok(Self {
            level,
            sender,
            dropped: 0,
        })
    }

    pub(crate) fn emit(&mut self, level: Level, message: fmt::Arguments<'_>) {
        if level > self.level {
            return;
        }
        let mut text = BoundedText(String::new());
        if fmt::write(&mut text, message).is_err() {
            text.0.push_str(" [truncated]");
        }
        if self
            .sender
            .try_send(Message::Record(level, text.0))
            .is_err()
        {
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    /// Called after workload teardown. A stuck diagnostic sink cannot hold the
    /// retained result or exit helper indefinitely; its failure is retained.
    pub(crate) fn flush(&mut self) -> Vec<String> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut errors = if self.sender.try_send(Message::Flush(sender)).is_err() {
            vec!["diagnostics queue unavailable during flush".into()]
        } else {
            match receiver.recv_timeout(FLUSH_DEADLINE) {
                Ok(errors) => errors,
                Err(error) => vec![format!(
                    "diagnostics flush deadline/channel failure: {error}"
                )],
            }
        };
        if self.dropped != 0 {
            errors.push(format!("diagnostics dropped {} records", self.dropped));
            self.dropped = 0;
        }
        errors
    }
}

struct BoundedText(String);

impl fmt::Write for BoundedText {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let remaining = MESSAGE_BYTES.saturating_sub(self.0.len());
        let mut length = value.len().min(remaining);
        while !value.is_char_boundary(length) {
            length -= 1;
        }
        self.0.push_str(&value[..length]);
        if length < value.len() {
            Err(fmt::Error)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_formatting_is_bounded_at_a_utf8_boundary() {
        let mut text = BoundedText(String::new());
        assert!(fmt::write(&mut text, format_args!("{}", "🦔".repeat(100_000))).is_err());
        assert!(text.0.len() <= MESSAGE_BYTES);
        assert!(text.0.ends_with('🦔'));
    }
}
