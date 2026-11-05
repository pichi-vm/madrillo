//! Bind access to the invoking account, even when Windows chooses a group owner.
use interprocess::os::windows::security_descriptor::SecurityDescriptor;
use std::io;

/// Require private, inheritable permissions before creating any retained state.
/// Administrators and SYSTEM are the Windows equivalents of a trusted root.
pub(crate) fn state_parent(path: &std::path::Path) -> io::Result<cap_std::fs::Dir> {
    use win_custody::{ObjectSecurityDescriptor, OwnedSid};
    let directory = cap_std::fs::Dir::open_ambient_dir(path, cap_std::ambient_authority())?;
    let descriptor = ObjectSecurityDescriptor::query_kernel_object(&directory)?;
    let user = OwnedSid::from_string(&win_custody::current_user_sid_string()?)?;
    let system = OwnedSid::from_string("S-1-5-18")?;
    let administrators = OwnedSid::from_string("S-1-5-32-544")?;
    let creator = OwnedSid::from_string("S-1-3-0")?;
    let trusted = [&user, &system, &administrators];
    let owner = descriptor.owner().map_err(io::Error::other)?;
    let acl = descriptor.dacl().map_err(io::Error::other)?;
    let full_access = |mask: u32| mask & 0x1000_0000 != 0 || mask & 0x001f_01ff == 0x001f_01ff;
    let private = trusted.iter().any(|sid| owner.matches(sid))
        && acl.allowed_aces().all(|ace| {
            ace.mask() == 0
                || trusted.iter().any(|sid| ace.trustee_matches(sid))
                || (ace.trustee_matches(&creator) && ace.flags() & 0x08 != 0)
        })
        && acl.allowed_aces().any(|ace| {
            // OBJECT_INHERIT | CONTAINER_INHERIT, without NO_PROPAGATE_INHERIT.
            ace.flags() & 0x07 == 0x03
                && full_access(ace.mask())
                && (ace.trustee_matches(&user) || ace.trustee_matches(&creator))
        });
    if !private {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Windows state parent must be owned by the user/SYSTEM/Administrators, grant access only to them, and inherit full user access to directories and files",
        ));
    }
    Ok(directory)
}

pub(crate) fn pipe_descriptor() -> io::Result<SecurityDescriptor> {
    let sid = win_custody::current_user_sid_string()?;
    let sddl = widestring::U16CString::from_str(format!("D:P(A;;GA;;;{sid})"))
        .map_err(io::Error::other)?;
    SecurityDescriptor::deserialize(&sddl)
}
