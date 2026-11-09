//! Protect only this monitor. The fresh launcher restores the caller's score.
#[cfg(target_os = "linux")]
pub(crate) fn protect(
    diagnostics: &mut crate::diagnostics::Diagnostics,
) -> (Option<i32>, Option<String>) {
    use std::io::Read;
    let original = (|| -> std::io::Result<i32> {
        let mut value = String::new();
        std::fs::File::open("/proc/self/oom_score_adj")?
            .take(64)
            .read_to_string(&mut value)?;
        let value: i32 = value.trim().parse().map_err(std::io::Error::other)?;
        if !(-1000..=1000).contains(&value) {
            return Err(std::io::Error::other("invalid inherited OOM adjustment"));
        }
        Ok(value)
    })();
    let original = match original {
        Ok(original) => original,
        Err(error) => {
            return (None, Some(format!("OOM protection unavailable: {error}")));
        }
    };
    if let Err(error) = restore(-1000) {
        // Rootless callers normally cannot lower their inherited adjustment.
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            diagnostics.emit(
                crate::diagnostics::Level::Debug,
                format_args!("OOM protection unavailable: {error}"),
            );
            return (None, None);
        }
        return (None, Some(format!("OOM protection unavailable: {error}")));
    }
    (Some(original), None)
}

#[cfg(target_os = "linux")]
pub(crate) fn restore(original: i32) -> std::io::Result<()> {
    std::fs::write("/proc/self/oom_score_adj", original.to_string())
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn protect(
    _diagnostics: &mut crate::diagnostics::Diagnostics,
) -> (Option<i32>, Option<String>) {
    (None, None)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn restore(_original: i32) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}
