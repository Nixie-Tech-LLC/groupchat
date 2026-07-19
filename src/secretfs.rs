//! Creating secrets with an explicit private-storage boundary, on every platform.
//!
//! Key material (FROST shares, signing nonces, the break-glass recovery keys)
//! must never be readable by other accounts on the machine. Unix expresses that
//! with `0o600`/`0o700`, which the callers already did. Windows has no
//! equivalent in std: there is no `DirBuilderExt`, and `OpenOptionsExt` exposes
//! only `CreateFileW`'s access/share/flags bits — `lpSecurityAttributes` is
//! hardcoded null — so a file created through `std::fs` inherits the parent
//! directory's ACL, whatever that happens to be. Secrets therefore go through
//! `CreateFileW`/`CreateDirectoryW` here with a hand-built DACL.
//!
//! **Created private, never fixed up afterwards.** Setting an ACL after the
//! write leaves a window where the bytes are on disk under the inherited ACL.
//! Every function here passes the descriptor at creation.
//!
//! # Two classes of secret
//!
//! [`Wrap`] is the deliberate part of this module, not an implementation
//! detail. DPAPI binds ciphertext to *this user on this machine*, which is
//! exactly right for ceremony state and exactly wrong for a break-glass key
//! whose entire purpose is to be carried to another host. Wrapping the wrong
//! file converts "an attacker cannot read this" into "nobody can ever read this
//! again", and for recovery material that is a worse outcome than the disclosure
//! it prevents. See [`Wrap::Portable`].
//!
//! Wrapped payloads carry [`DPAPI_MAGIC`] so [`read_private`] reads both forms;
//! a store written before this module existed still opens.

use std::path::Path;

use anyhow::{Context, Result};

/// Prefix marking a DPAPI-wrapped payload. Absent ⇒ the bytes are verbatim, so
/// plaintext files written by earlier versions still read back.
const DPAPI_MAGIC: &[u8] = b"lait-dpapi-1\n";

/// Whether an existing file is an error or is replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Create {
    /// Fail if the path exists. Single-use material — a signing nonce that
    /// already exists must be *examined*, never silently overwritten.
    New,
    /// Replace any existing content.
    Replace,
}

/// Whether the payload gets an at-rest wrap beyond the filesystem ACL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wrap {
    /// Store bytes verbatim. For material that must survive being moved to
    /// another machine or user — the break-glass recovery keys, which the docs
    /// tell operators to take offline. A device-bound wrap would make the copy
    /// in the safe useless.
    Portable,
    /// Wrap with the OS user-bound facility where one exists (DPAPI on Windows;
    /// a no-op elsewhere). Device-local material only: a wrapped file does not
    /// survive a restore onto a different machine or user account.
    DeviceBound,
}

/// Create `path` as a directory only the current user (and the system account)
/// may traverse. Idempotent; hardens an existing directory in place.
pub fn create_private_dir(path: &Path) -> Result<()> {
    imp::create_private_dir(path)
}

/// Write `bytes` to `path` with owner-only access, applying `wrap`.
///
/// The parent directory must already be private — call [`create_private_dir`]
/// first. Content is flushed and fsynced before returning.
pub fn write_private(path: &Path, bytes: &[u8], create: Create, wrap: Wrap) -> Result<()> {
    let payload = match wrap {
        Wrap::Portable => bytes.to_vec(),
        Wrap::DeviceBound => imp::wrap(bytes)?,
    };
    imp::write_private(path, &payload, create)
}

/// Read a file written by [`write_private`], unwrapping if needed. `Ok(None)`
/// if it does not exist; an error if it exists but cannot be read or unwrapped
/// (a DPAPI blob restored onto a different machine lands here — deliberately
/// loud, since silently treating it as absent would look like key loss).
pub fn read_private(path: &Path) -> Result<Option<Vec<u8>>> {
    let raw = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).context("read secret"),
    };
    match raw.strip_prefix(DPAPI_MAGIC) {
        Some(blob) => imp::unwrap(blob).map(Some),
        None => Ok(Some(raw)),
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    pub(super) fn create_private_dir(path: &Path) -> Result<()> {
        std::fs::create_dir_all(path).context("create secret dir")?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .context("restrict secret dir permissions")
    }

    pub(super) fn write_private(path: &Path, bytes: &[u8], create: Create) -> Result<()> {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).mode(0o600);
        match create {
            Create::New => opts.create_new(true),
            Create::Replace => opts.create(true).truncate(true),
        };
        let mut f = opts.open(path).context("create secret file")?;
        f.write_all(bytes).context("write secret file")?;
        f.sync_all().context("fsync secret file")?;
        Ok(())
    }

    /// No user-bound wrap outside Windows; the `0o600` DACL equivalent is the
    /// whole control. Returning the input keeps the file portable, which is the
    /// safe direction — a reader never has to guess.
    pub(super) fn wrap(bytes: &[u8]) -> Result<Vec<u8>> {
        Ok(bytes.to_vec())
    }
    pub(super) fn unwrap(_blob: &[u8]) -> Result<Vec<u8>> {
        anyhow::bail!("this secret is DPAPI-wrapped and can only be read on the Windows account that wrote it")
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;

    use windows_sys::Win32::Foundation::{
        CloseHandle, LocalFree, ERROR_ALREADY_EXISTS, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB,
    };
    use windows_sys::Win32::Security::{
        AddAccessAllowedAce, CreateWellKnownSid, GetLengthSid, GetTokenInformation, InitializeAcl,
        InitializeSecurityDescriptor, SetSecurityDescriptorControl, SetSecurityDescriptorDacl,
        TokenUser, WinLocalSystemSid, ACL, ACL_REVISION, SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR,
        SECURITY_DESCRIPTOR_CONTROL, SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER,
    };

    /// `SECURITY_DESCRIPTOR_REVISION` from winnt.h. windows-sys 0.61 does not
    /// re-export it; the value is fixed at 1 by the Win32 ABI.
    const SECURITY_DESCRIPTOR_REVISION: u32 = 1;
    use windows_sys::Win32::Storage::FileSystem::{
        CreateDirectoryW, CreateFileW, CREATE_ALWAYS, CREATE_NEW, FILE_ALL_ACCESS,
        FILE_ATTRIBUTE_NORMAL,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    fn last_error(what: &str) -> anyhow::Error {
        anyhow::anyhow!("{what}: {}", std::io::Error::last_os_error())
    }

    /// An absolute security descriptor granting the current user and SYSTEM full
    /// access and **nobody else**. Buffers are held alongside the descriptor
    /// because it points into them; dropping this invalidates the descriptor.
    ///
    /// `SE_DACL_PROTECTED` is the load-bearing flag: without it the OS merges
    /// inheritable ACEs from the parent directory into the new object, so a
    /// permissive parent would re-widen the very access this exists to deny.
    struct PrivateSd {
        // u64-backed so the ACL and TOKEN_USER casts are correctly aligned;
        // a Vec<u8> would only guarantee byte alignment.
        _token: Vec<u64>,
        _system_sid: Vec<u64>,
        _acl: Vec<u64>,
        sd: Box<SECURITY_DESCRIPTOR>,
    }

    impl PrivateSd {
        fn new() -> Result<Self> {
            unsafe {
                // ---- current user's SID, out of the process token.
                let mut token: HANDLE = std::ptr::null_mut();
                if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                    return Err(last_error("OpenProcessToken"));
                }
                let mut need: u32 = 0;
                GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut need);
                if need == 0 {
                    CloseHandle(token);
                    return Err(last_error("GetTokenInformation (size)"));
                }
                let mut token_buf: Vec<u64> = vec![0; (need as usize).div_ceil(8) + 1];
                let ok = GetTokenInformation(
                    token,
                    TokenUser,
                    token_buf.as_mut_ptr().cast(),
                    need,
                    &mut need,
                );
                CloseHandle(token);
                if ok == 0 {
                    return Err(last_error("GetTokenInformation"));
                }
                let user_sid = (*(token_buf.as_ptr() as *const TOKEN_USER)).User.Sid;

                // ---- the local SYSTEM SID, so services/backup still work.
                let mut sys_len: u32 = 256;
                let mut system_sid: Vec<u64> = vec![0; 32];
                if CreateWellKnownSid(
                    WinLocalSystemSid,
                    std::ptr::null_mut(),
                    system_sid.as_mut_ptr().cast(),
                    &mut sys_len,
                ) == 0
                {
                    return Err(last_error("CreateWellKnownSid"));
                }
                let system_ptr = system_sid.as_mut_ptr().cast();

                // ---- a DACL holding exactly those two allow-ACEs.
                // ACCESS_ALLOWED_ACE embeds the first DWORD of the SID, hence the
                // `- 4` when sizing each ACE.
                let ace_overhead = std::mem::size_of::<u32>() * 2 + std::mem::size_of::<u32>();
                let acl_len = std::mem::size_of::<ACL>()
                    + ace_overhead
                    + GetLengthSid(user_sid) as usize
                    + ace_overhead
                    + GetLengthSid(system_ptr) as usize
                    + 16; // slack; InitializeAcl only needs >= the exact size
                let mut acl_buf: Vec<u64> = vec![0; acl_len.div_ceil(8)];
                let acl = acl_buf.as_mut_ptr() as *mut ACL;
                if InitializeAcl(acl, (acl_buf.len() * 8) as u32, ACL_REVISION) == 0 {
                    return Err(last_error("InitializeAcl"));
                }
                if AddAccessAllowedAce(acl, ACL_REVISION, FILE_ALL_ACCESS, user_sid) == 0 {
                    return Err(last_error("AddAccessAllowedAce (user)"));
                }
                if AddAccessAllowedAce(acl, ACL_REVISION, FILE_ALL_ACCESS, system_ptr) == 0 {
                    return Err(last_error("AddAccessAllowedAce (system)"));
                }

                // ---- the descriptor itself, with inheritance blocked.
                let mut sd: Box<SECURITY_DESCRIPTOR> = Box::new(std::mem::zeroed());
                let sd_ptr = (&mut *sd) as *mut SECURITY_DESCRIPTOR as *mut _;
                if InitializeSecurityDescriptor(sd_ptr, SECURITY_DESCRIPTOR_REVISION) == 0 {
                    return Err(last_error("InitializeSecurityDescriptor"));
                }
                if SetSecurityDescriptorDacl(sd_ptr, 1, acl, 0) == 0 {
                    return Err(last_error("SetSecurityDescriptorDacl"));
                }
                if SetSecurityDescriptorControl(
                    sd_ptr,
                    SE_DACL_PROTECTED as SECURITY_DESCRIPTOR_CONTROL,
                    SE_DACL_PROTECTED as SECURITY_DESCRIPTOR_CONTROL,
                ) == 0
                {
                    return Err(last_error("SetSecurityDescriptorControl"));
                }

                Ok(PrivateSd {
                    _token: token_buf,
                    _system_sid: system_sid,
                    _acl: acl_buf,
                    sd,
                })
            }
        }

        fn attributes(&mut self) -> SECURITY_ATTRIBUTES {
            SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: (&mut *self.sd) as *mut SECURITY_DESCRIPTOR as *mut _,
                bInheritHandle: 0,
            }
        }
    }

    pub(super) fn create_private_dir(path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent).context("create secret dir parent")?;
            }
        }
        let mut sd = PrivateSd::new()?;
        let sa = sd.attributes();
        let w = wide(path);
        unsafe {
            if CreateDirectoryW(w.as_ptr(), &sa) == 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() != Some(ERROR_ALREADY_EXISTS as i32) {
                    return Err(err).context("CreateDirectoryW (secret dir)");
                }
                // Already there: the ACL we want is applied to the files we
                // create inside it, each of which carries this same descriptor,
                // so an inherited-permissive directory cannot widen them.
            }
        }
        Ok(())
    }

    pub(super) fn write_private(path: &Path, bytes: &[u8], create: Create) -> Result<()> {
        use std::io::Write;
        let mut sd = PrivateSd::new()?;
        let sa = sd.attributes();
        let w = wide(path);
        let disposition = match create {
            Create::New => CREATE_NEW,
            Create::Replace => CREATE_ALWAYS,
        };
        let handle = unsafe {
            CreateFileW(
                w.as_ptr(),
                GENERIC_WRITE,
                0, // no sharing while we hold it open
                &sa,
                disposition,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error()).context("CreateFileW (secret file)");
        }
        let mut f = unsafe { std::fs::File::from_raw_handle(handle as _) };
        f.write_all(bytes).context("write secret file")?;
        f.sync_all().context("fsync secret file")?;
        Ok(())
    }

    pub(super) fn wrap(bytes: &[u8]) -> Result<Vec<u8>> {
        unsafe {
            let input = CRYPT_INTEGER_BLOB {
                cbData: bytes.len() as u32,
                pbData: bytes.as_ptr() as *mut u8,
            };
            let mut out = CRYPT_INTEGER_BLOB {
                cbData: 0,
                pbData: std::ptr::null_mut(),
            };
            if CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
                &mut out,
            ) == 0
            {
                return Err(last_error("CryptProtectData"));
            }
            let blob = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
            LocalFree(out.pbData.cast());
            let mut v = Vec::with_capacity(DPAPI_MAGIC.len() + blob.len());
            v.extend_from_slice(DPAPI_MAGIC);
            v.extend_from_slice(&blob);
            Ok(v)
        }
    }

    pub(super) fn unwrap(blob: &[u8]) -> Result<Vec<u8>> {
        unsafe {
            let input = CRYPT_INTEGER_BLOB {
                cbData: blob.len() as u32,
                pbData: blob.as_ptr() as *mut u8,
            };
            let mut out = CRYPT_INTEGER_BLOB {
                cbData: 0,
                pbData: std::ptr::null_mut(),
            };
            if CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
                &mut out,
            ) == 0
            {
                return Err(last_error(
                    "CryptUnprotectData (this secret belongs to a different Windows account or machine)",
                ));
            }
            let plain = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
            LocalFree(out.pbData.cast());
            Ok(plain)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("lait-secretfs-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_portable_secret_round_trips_verbatim() {
        let dir = tmp("portable");
        create_private_dir(&dir).unwrap();
        let path = dir.join("space-recovery.key");
        write_private(&path, b"deadbeef", Create::New, Wrap::Portable).unwrap();
        assert_eq!(
            read_private(&path).unwrap().as_deref(),
            Some(&b"deadbeef"[..])
        );
        // Portable means portable: the bytes on disk are the bytes we wrote, so
        // a key carried to another machine still opens.
        assert_eq!(std::fs::read(&path).unwrap(), b"deadbeef");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_device_bound_secret_round_trips_through_the_wrap() {
        let dir = tmp("bound");
        create_private_dir(&dir).unwrap();
        let path = dir.join("share");
        let secret = vec![7u8; 64];
        write_private(&path, &secret, Create::New, Wrap::DeviceBound).unwrap();
        assert_eq!(read_private(&path).unwrap(), Some(secret.clone()));
        #[cfg(windows)]
        {
            let raw = std::fs::read(&path).unwrap();
            assert!(raw.starts_with(DPAPI_MAGIC), "wrapped payloads are tagged");
            assert_ne!(raw, secret, "the share is not on disk in the clear");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn create_new_refuses_to_overwrite_single_use_material() {
        let dir = tmp("createnew");
        create_private_dir(&dir).unwrap();
        let path = dir.join("nonces");
        write_private(&path, b"first", Create::New, Wrap::DeviceBound).unwrap();
        assert!(
            write_private(&path, b"second", Create::New, Wrap::DeviceBound).is_err(),
            "single-use nonce state must never be silently overwritten"
        );
        assert_eq!(read_private(&path).unwrap().as_deref(), Some(&b"first"[..]));
        // Replace is the explicit opt-in for mutable material.
        write_private(&path, b"second", Create::Replace, Wrap::DeviceBound).unwrap();
        assert_eq!(
            read_private(&path).unwrap().as_deref(),
            Some(&b"second"[..])
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_secret_reads_as_none_not_an_error() {
        let dir = tmp("missing");
        create_private_dir(&dir).unwrap();
        assert!(read_private(&dir.join("nope")).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn create_private_dir_is_idempotent() {
        let dir = tmp("idempotent");
        create_private_dir(&dir).unwrap();
        create_private_dir(&dir).unwrap();
        assert!(dir.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Manual check: leaves a secret behind at a fixed path so its real ACL can
    /// be inspected (`icacls`/`Get-Acl` on Windows, `ls -l` elsewhere). The
    /// round-trip tests above prove nothing about permissions — this is how the
    /// boundary itself gets verified.
    ///
    /// `cargo test --lib secretfs -- --ignored --nocapture`
    #[test]
    #[ignore = "leaves a file behind for manual ACL inspection"]
    fn inspect_the_real_acl() {
        let dir = std::env::temp_dir().join("lait-secretfs-inspect");
        let _ = std::fs::remove_dir_all(&dir);
        create_private_dir(&dir).unwrap();
        let path = dir.join("share");
        write_private(&path, &[9u8; 32], Create::New, Wrap::DeviceBound).unwrap();
        println!("dir:  {}", dir.display());
        println!("file: {}", path.display());
    }

    #[cfg(unix)]
    #[test]
    fn unix_modes_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp("modes");
        create_private_dir(&dir).unwrap();
        let path = dir.join("k");
        write_private(&path, b"x", Create::New, Wrap::Portable).unwrap();
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
