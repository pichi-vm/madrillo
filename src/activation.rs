//! Standard inherited socket activation, owned by the actual workload process.
use std::{ffi::OsString, io, process::Command};

pub(crate) struct Activation {
    count: usize,
    names: Option<OsString>,
}
impl Activation {
    pub(crate) fn discover(replace_pid: bool) -> io::Result<Option<Self>> {
        #[cfg(not(target_os = "linux"))]
        {
            if replace_pid {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "systemd socket activation requires Linux",
                ));
            }
            Ok(None)
        }
        #[cfg(target_os = "linux")]
        {
            let Some(pid) = std::env::var_os("LISTEN_PID") else {
                return Ok(None);
            };
            let pid: u32 = pid
                .to_str()
                .ok_or_else(|| io::Error::other("invalid LISTEN_PID"))?
                .parse()
                .map_err(io::Error::other)?;
            if pid == 0 {
                return Err(io::Error::other("invalid LISTEN_PID"));
            }
            if !replace_pid && pid != std::process::id() {
                return Ok(None);
            }
            let Some(count) = std::env::var_os("LISTEN_FDS") else {
                return Ok(None);
            };
            let count = count
                .to_str()
                .ok_or_else(|| io::Error::other("invalid LISTEN_FDS"))?
                .parse::<usize>()
                .map_err(io::Error::other)?;
            if count == 0 || count > 64 {
                return Err(io::Error::other("LISTEN_FDS must be in 1..=64"));
            }
            let names = std::env::var_os("LISTEN_FDNAMES");
            if let Some(names) = &names {
                let names = names.as_encoded_bytes();
                if names.len() > 4096 || names.split(|byte| *byte == b':').count() != count {
                    return Err(io::Error::other("LISTEN_FDNAMES does not match LISTEN_FDS"));
                }
            }
            Ok(Some(Self { count, names }))
        }
    }
    pub(crate) fn numbers(&self) -> std::ops::Range<usize> {
        3..3 + self.count
    }
    pub(crate) fn configure(&self, command: &mut Command) {
        command
            .env("LISTEN_PID", std::process::id().to_string())
            .env("LISTEN_FDS", self.count.to_string());
        if let Some(names) = &self.names {
            command.env("LISTEN_FDNAMES", names);
        }
    }
}

pub(crate) fn clear(command: &mut Command) {
    command
        .env_remove("LISTEN_PID")
        .env_remove("LISTEN_PIDFDID")
        .env_remove("LISTEN_FDS")
        .env_remove("LISTEN_FDNAMES");
}
