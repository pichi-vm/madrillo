#![forbid(unsafe_code)]

//! Shared protocol and retained-result types. Monitor implementation belongs
//! to the executable; this library has no transport or process runtime.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u16,
    pub message: ClientMessage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientMessage {
    Attach { input: bool, streams: Vec<String> },
    Start,
    Input { bytes: Vec<u8> },
    InputEnd,
    CloseInput,
    Resize { rows: u16, cols: u16 },
    ReopenLogs,
    Wait,
    Terminate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub version: u16,
    pub message: ServerMessage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ServerMessage {
    Prepared { monitor_pid: u32 },
    Started { leader_pid: u32 },
    Attached { input: bool },
    LogsReopened,
    Output { stream: String, bytes: Vec<u8> },
    Completed { result: Completion },
    Error { code: ErrorCode, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Version,
    InvalidRequest,
    InputOwned,
    InputClosed,
    Unsupported,
    NotRunning,
    Io,
    ResourceLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Completion {
    pub version: u16,
    pub leader: LeaderResult,
    pub cause: CompletionCause,
    pub exited_at_unix_ms: u64,
    pub cleanup: CleanupResult,
    pub oom: OomEvidence,
    pub log_errors: Vec<String>,
    pub monitor_errors: Vec<String>,
    pub monitor_warnings: Vec<String>,
    pub output_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LeaderResult {
    Exit { code: u32 },
    Signal { signal: u32, core_dumped: bool },
    SpawnFailed { message: String },
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionCause {
    LeaderExit,
    Forced,
    TimedOut,
    LaunchCancelled,
    LaunchFailed,
    MonitorFailure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CleanupResult {
    Complete,
    Incomplete { errors: Vec<String> },
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum OomEvidence {
    Observed { events: u64, kills: u64 },
    NoneObserved,
    Unavailable { reason: String },
}
