//! Files only the signed-in Windows or Unix user can open, for secrets the
//! app keeps outside the keyring (the Hermes agent keys, PR C2).
//!
//! - Unix: created `0o600` before any byte is written
//!   ([`crate::managed_agents::atomic_write_json_restricted`]).
//! - Windows: a new file whose protected DACL grants the current user alone
//!   (no inherited entries, so not SYSTEM or Administrators either), set on
//!   the open handle before any byte is written. It is then renamed over
//!   the target, keeping that DACL.

use std::path::Path;

/// Replace `path` with `payload`, readable and writable by the current user
/// only.
pub(crate) fn write_owner_only(path: &Path, payload: &[u8]) -> Result<(), String> {
    #[cfg(windows)]
    {
        windows::write_owner_only(path, payload)
    }
    #[cfg(not(windows))]
    {
        crate::managed_agents::atomic_write_json_restricted(path, payload)
    }
}

#[cfg(windows)]
mod windows {
    use std::io::Write as _;
    use std::os::windows::ffi::OsStrExt as _;
    use std::os::windows::fs::OpenOptionsExt as _;
    use std::os::windows::io::AsRawHandle as _;
    use std::path::Path;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        GetSecurityDescriptorDacl, GetTokenInformation, TokenUser, ACL, DACL_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    const GENERIC_WRITE: u32 = 0x4000_0000;
    const WRITE_DAC: u32 = 0x0004_0000;

    pub(super) fn write_owner_only(path: &Path, payload: &[u8]) -> Result<(), String> {
        let name = path
            .file_name()
            .ok_or_else(|| format!("{} is not a file path", path.display()))?;
        let mut temp_name = name.to_os_string();
        temp_name.push(format!(".tmp-{}", uuid::Uuid::new_v4().simple()));
        let temp = path.with_file_name(temp_name);
        let written = write_new(&temp, payload)
            .and_then(|()| std::fs::rename(&temp, path).map_err(|e| format!("replace file: {e}")));
        if written.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        written
    }

    fn write_new(path: &Path, payload: &[u8]) -> Result<(), String> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .access_mode(GENERIC_WRITE | WRITE_DAC)
            .open(path)
            .map_err(|e| format!("create file: {e}"))?;
        restrict_to_current_user(file.as_raw_handle() as HANDLE)?;
        file.write_all(payload)
            .map_err(|e| format!("write file: {e}"))?;
        file.sync_all().map_err(|e| format!("flush file: {e}"))
    }

    /// The current user's SID, e.g. `S-1-5-21-…`.
    fn current_user_sid() -> Result<String, String> {
        // SAFETY: plain Win32 calls on handles and buffers owned here; the
        // token handle and the SID string are released before returning.
        unsafe {
            let mut token: HANDLE = null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err("read the current user".into());
            }
            let mut len = 0u32;
            GetTokenInformation(token, TokenUser, null_mut(), 0, &mut len);
            // u64 words keep TOKEN_USER (which holds a pointer) aligned.
            let mut buffer = vec![0u64; (len as usize).div_ceil(8).max(1)];
            let ok =
                GetTokenInformation(token, TokenUser, buffer.as_mut_ptr().cast(), len, &mut len);
            CloseHandle(token);
            if ok == 0 {
                return Err("read the current user".into());
            }
            let user = &*(buffer.as_ptr() as *const TOKEN_USER);
            let mut text = null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 || text.is_null() {
                return Err("read the current user".into());
            }
            let mut end = 0usize;
            while *text.add(end) != 0 {
                end += 1;
            }
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, end));
            LocalFree(text.cast());
            Ok(sid)
        }
    }

    /// Give the file at `handle` a protected DACL with one entry: full
    /// access for the current user.
    fn restrict_to_current_user(handle: HANDLE) -> Result<(), String> {
        let sddl: Vec<u16> = std::ffi::OsStr::new(&format!("D:P(A;;FA;;;{})", current_user_sid()?))
            .encode_wide()
            .chain(Some(0))
            .collect();
        // SAFETY: `sddl` is NUL-terminated; the descriptor is freed below
        // and its DACL is used only while it lives.
        unsafe {
            let mut descriptor = null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            ) == 0
            {
                return Err("build the file permissions".into());
            }
            let mut present = 0;
            let mut defaulted = 0;
            let mut dacl: *mut ACL = null_mut();
            let read =
                GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted);
            let result = if read == 0 || present == 0 {
                Err("build the file permissions".to_string())
            } else {
                match SetSecurityInfo(
                    handle,
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    dacl,
                    null(),
                ) {
                    0 => Ok(()),
                    code => Err(format!("set the file permissions (error {code})")),
                }
            };
            LocalFree(descriptor);
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_only_file_is_written_and_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.json");
        write_owner_only(&path, b"one").unwrap();
        write_owner_only(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        // No temporary file is left behind.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn owner_only_file_is_0600_on_unix() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.json");
        write_owner_only(&path, b"x").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[cfg(windows)]
    #[test]
    fn owner_only_file_has_one_uninherited_entry_on_windows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.json");
        write_owner_only(&path, b"x").unwrap();
        let mut icacls = std::process::Command::new("icacls");
        icacls.arg(&path);
        let output =
            crate::managed_agents::output_with_timeout(icacls, std::time::Duration::from_secs(10))
                .expect("icacls runs");
        let text = String::from_utf8_lossy(&output.stdout);
        // `<path> DOMAIN\user:(F)`: one entry, nothing inherited.
        let entries = text.matches(":(").count();
        assert_eq!(entries, 1, "{text}");
        assert!(text.contains(":(F)"), "{text}");
        assert!(!text.contains("(I)"), "{text}");
    }
}
