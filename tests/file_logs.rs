#![forbid(unsafe_code)]
#![cfg(feature = "cli")]
#[path = "../src/logging/cri.rs"]
mod cri;
#[path = "../src/logging/file.rs"]
mod file;
use chrono::DateTime;
use cri::Stream;
use file::{FileLog, Limits};
use std::fs;

#[test]
fn rotation_retains_whole_records_and_cumulative_cap_survives_reopen()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("log");
    let time = DateTime::parse_from_rfc3339("2026-11-05T12:00:00.123456789+00:00")?;
    let mut log = FileLog::open(
        &path,
        Limits {
            size: Some(50),
            total: Some(100),
            backups: Some(2),
        },
        true,
    )?;
    // Each record is 49 bytes. The cumulative cap is checked before a record,
    // so the third record takes total written bytes past 100; the fourth drops.
    log.write_chunk(time, Stream::Stdout, b"one\ntwo\ntri\nend\n")?;
    log.sync()?;
    assert!(fs::read_to_string(&path)?.ends_with("stdout F tri\n"));
    assert!(fs::read_to_string(path.with_file_name("log.1"))?.ends_with("stdout F two\n"));
    assert!(fs::read_to_string(path.with_file_name("log.2"))?.ends_with("stdout F one\n"));
    log.reopen()?;
    log.write_chunk(time, Stream::Stderr, b"still capped\n")?;
    assert!(fs::read(path)?.is_empty());
    Ok(())
}

#[test]
fn oversized_records_are_whole_and_truncation_replaces_inode()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("log");
    let time = DateTime::parse_from_rfc3339("2026-11-05T12:00:00.123456789+00:00")?;
    let mut log = FileLog::open(
        &path,
        Limits {
            size: Some(1),
            ..Limits::default()
        },
        true,
    )?;
    log.write_chunk(time, Stream::Stdout, b"first\nsecond\n")?;
    let content = fs::read_to_string(path)?;
    assert_eq!(
        content,
        "2026-11-05T12:00:00.123456789+00:00 stdout F second\n"
    );
    Ok(())
}

#[test]
fn an_existing_log_is_appended_to() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("log");
    fs::write(&path, b"existing\n")?;
    let mut log = FileLog::open(&path, Limits::default(), true)?;
    log.write_chunk(
        DateTime::parse_from_rfc3339("2026-11-05T12:00:00.123456789+00:00")?,
        Stream::Stdout,
        b"new\n",
    )?;
    assert!(fs::read_to_string(path)?.starts_with("existing\n2026-"));
    Ok(())
}

#[cfg(unix)]
#[test]
fn special_files_and_links_are_rejected_without_modifying_targets()
-> Result<(), Box<dyn std::error::Error>> {
    use std::io;
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir()?;
    let target = root.path().join("target");
    fs::write(&target, b"preserved")?;
    let link = root.path().join("link");
    symlink(&target, &link)?;
    assert!(FileLog::open(&link, Limits::default(), true).is_err());
    let hard = root.path().join("hard");
    fs::hard_link(&target, &hard)?;
    assert!(FileLog::open(&hard, Limits::default(), true).is_err());
    let fifo = root.path().join("fifo");
    nix::unistd::mkfifo(
        &fifo,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )?;
    let error = FileLog::open(&fifo, Limits::default(), true)
        .err()
        .ok_or_else(|| io::Error::other("FIFO was accepted"))?;
    assert!(error.kind() == io::ErrorKind::Other || error.raw_os_error() == Some(nix::libc::ENXIO));
    assert_eq!(fs::read(target)?, b"preserved");
    Ok(())
}

#[test]
fn failed_rotation_preserves_active_log_and_removes_prepared_file()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("log");
    let mut log = FileLog::open(
        &path,
        Limits {
            backups: Some(1),
            ..Limits::default()
        },
        true,
    )?;
    let time = DateTime::parse_from_rfc3339("2026-11-05T12:00:00.123456789+00:00")?;
    log.write_chunk(time, Stream::Stdout, b"before\n")?;
    let before = fs::read(&path)?;
    fs::create_dir(root.path().join("log.1"))?;
    assert!(log.reopen().is_err());
    assert_eq!(fs::read(&path)?, before);
    log.write_chunk(time, Stream::Stdout, b"after\n")?;
    assert!(fs::read_to_string(&path)?.ends_with("stdout F after\n"));
    assert_eq!(fs::read_dir(root.path())?.count(), 2);
    Ok(())
}

#[cfg(windows)]
#[test]
fn hard_links_are_rejected_on_windows() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let target = root.path().join("target");
    fs::write(&target, b"preserved")?;
    let link = root.path().join("link");
    fs::hard_link(&target, &link)?;
    assert!(FileLog::open(&link, Limits::default(), true).is_err());
    assert_eq!(fs::read(target)?, b"preserved");
    Ok(())
}

#[test]
fn global_cap_counts_bytes_already_in_the_active_file() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("log");
    fs::write(&path, b"existing contents")?;
    let mut log = FileLog::open(
        &path,
        Limits {
            total: Some(1),
            ..Limits::default()
        },
        true,
    )?;
    log.write_chunk(
        DateTime::parse_from_rfc3339("2026-11-05T12:00:00.123456789+00:00")?,
        Stream::Stdout,
        b"not retained\n",
    )?;
    assert_eq!(fs::read(&path)?, b"existing contents");
    Ok(())
}
