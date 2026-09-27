use std::{ffi::OsStr, io, os::windows::ffi::OsStrExt, path::Path, ptr};

use windows_sys::Win32::{
    Foundation::{GetLastError, LocalFree},
    Security::{
        Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW,
        DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, SetFileSecurityW,
    },
};

const SDDL_REVISION_1: u32 = 1;

/// Replaces inherited access on a sensitive per-user directory with a DACL
/// that admits the owner, local administrators, and SYSTEM. Child files and
/// directories inherit the same restriction.
pub fn protect_private_directory(path: &Path) -> io::Result<()> {
    let path = wide(path.as_os_str());
    // OW is the Windows Owner Rights SID. CO ensures children created under
    // another effective owner still receive an owner-specific inheritable ACE.
    let sddl = wide(OsStr::new(
        "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;OW)(A;OICIIO;FA;;;CO)",
    ));
    let mut descriptor = ptr::null_mut();
    // SAFETY: both strings are NUL terminated and `descriptor` is released by
    // LocalFree on every path after a successful conversion.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the descriptor was allocated by the conversion API and remains
    // live for the SetFileSecurityW call.
    let applied = unsafe {
        SetFileSecurityW(
            path.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        )
    };
    let error = (applied == 0).then(|| {
        // Capture the code before LocalFree can disturb thread-local state.
        io::Error::from_raw_os_error(unsafe { GetLastError() } as i32)
    });
    // SAFETY: ownership of the descriptor is returned to the Windows local
    // allocator exactly once.
    unsafe {
        LocalFree(descriptor as _);
    }
    error.map_or(Ok(()), Err)
}

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use std::{fs, ptr};

    use tempfile::tempdir;
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, GetFileSecurityW, GetSecurityDescriptorControl,
        SE_DACL_PROTECTED,
    };

    use super::{protect_private_directory, wide};

    #[test]
    fn directory_dacl_is_protected_and_owner_can_create_children() {
        let directory = tempdir().unwrap();
        protect_private_directory(directory.path()).unwrap();
        fs::write(directory.path().join("probe"), b"private").unwrap();

        let path = wide(directory.path().as_os_str());
        let mut needed = 0;
        // SAFETY: the null-buffer probe is the documented sizing call.
        unsafe {
            GetFileSecurityW(
                path.as_ptr(),
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                0,
                &mut needed,
            );
        }
        assert!(needed > 0);
        let mut descriptor = vec![0_u8; needed as usize];
        // SAFETY: the descriptor buffer has the exact size requested above.
        assert_ne!(
            unsafe {
                GetFileSecurityW(
                    path.as_ptr(),
                    DACL_SECURITY_INFORMATION,
                    descriptor.as_mut_ptr().cast(),
                    needed,
                    &mut needed,
                )
            },
            0
        );
        let mut control = 0;
        let mut revision = 0;
        // SAFETY: the buffer contains a security descriptor returned by Win32.
        assert_ne!(
            unsafe {
                GetSecurityDescriptorControl(
                    descriptor.as_mut_ptr().cast(),
                    &mut control,
                    &mut revision,
                )
            },
            0
        );
        assert_ne!(control & SE_DACL_PROTECTED, 0);
    }
}
