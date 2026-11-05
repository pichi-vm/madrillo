//! Bind access to the invoking account, even when Windows chooses a group owner.
use interprocess::os::windows::security_descriptor::SecurityDescriptor;
use std::io;

pub(crate) fn pipe_descriptor() -> io::Result<SecurityDescriptor> {
    let sid = win_custody::current_user_sid_string()?;
    let sddl = widestring::U16CString::from_str(format!("D:P(A;;GA;;;{sid})"))
        .map_err(io::Error::other)?;
    SecurityDescriptor::deserialize(&sddl)
}
