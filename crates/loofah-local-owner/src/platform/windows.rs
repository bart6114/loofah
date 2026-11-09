use super::Stream;
use crate::{Error, Result};
use std::{
    ffi::c_void,
    os::windows::{ffi::OsStrExt, fs::MetadataExt, io::AsRawHandle},
    path::{Path, PathBuf},
};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree},
        Security::{
            ACCESS_ALLOWED_ACE, ACL,
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                GetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
            },
            DACL_SECURITY_INFORMATION, GetAce, GetTokenInformation, IsValidAcl, IsValidSid,
            OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
            TOKEN_QUERY, TOKEN_USER, TokenUser,
        },
        Storage::FileSystem::CreateDirectoryW,
        System::{
            Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId},
            Threading::{
                GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
    },
    core::{PCWSTR, PWSTR},
};

pub(crate) type Address = String;
pub(crate) struct Listener {
    pending: Option<NamedPipeServer>,
    name: String,
    sid: String,
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
struct Allocation(*mut c_void);
impl Drop for Allocation {
    fn drop(&mut self) {
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.0)));
        }
    }
}

fn win(cause: windows::core::Error) -> Error {
    std::io::Error::from_raw_os_error(cause.code().0 & 0xffff).into()
}
fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

fn sid_string(sid: PSID) -> Result<String> {
    // Sid is owned by the live Windows token/security-descriptor allocation at every call.
    unsafe {
        if !IsValidSid(sid).as_bool() {
            return Err(Error::protocol("Invalid Windows owner SID"));
        }
        let mut text = PWSTR::null();
        ConvertSidToStringSidW(sid, &mut text).map_err(win)?;
        let _allocation = Allocation(text.0.cast());
        text.to_string()
            .map_err(|_| Error::protocol("Invalid Windows owner SID string"))
    }
}

fn process_sid(process: HANDLE) -> Result<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token).map_err(win)?;
        let token = Handle(token);
        let mut size = 0;
        let _ = GetTokenInformation(token.0, TokenUser, None, 0, &mut size);
        if size < std::mem::size_of::<TOKEN_USER>() as u32 || size > 64 * 1024 {
            return Err(Error::protocol("Invalid Windows token size"));
        }
        // usize storage supplies TOKEN_USER alignment on both supported Windows architectures.
        let mut storage = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
        GetTokenInformation(
            token.0,
            TokenUser,
            Some(storage.as_mut_ptr().cast()),
            size,
            &mut size,
        )
        .map_err(win)?;
        let user = &*storage.as_ptr().cast::<TOKEN_USER>();
        sid_string(user.User.Sid)
    }
}
fn current_sid() -> Result<String> {
    unsafe { process_sid(GetCurrentProcess()) }
}

struct Security {
    allocation: Allocation,
    descriptor: PSECURITY_DESCRIPTOR,
}
impl Security {
    fn new(sid: &str, directory: bool) -> Result<Self> {
        let flags = if directory { "OICI" } else { "" };
        let text = wide(std::ffi::OsStr::new(&format!(
            "O:{sid}D:P(A;{flags};GA;;;{sid})"
        )));
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(text.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
            .map_err(win)?;
        }
        Ok(Self {
            allocation: Allocation(descriptor.0),
            descriptor,
        })
    }
    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        let _ = &self.allocation;
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor.0,
            bInheritHandle: false.into(),
        }
    }
}

pub(super) fn system_root() -> Result<PathBuf> {
    let sid = current_sid()?;
    Ok(std::env::temp_dir().canonicalize()?.join(format!(
        "loofah-owner-{}",
        &super::hash(sid.as_bytes())[..16]
    )))
}

pub(super) fn ensure_root(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        return Err(Error::protocol("Local owner directory must be absolute"));
    }
    let sid = current_sid()?;
    if !path.try_exists()? {
        let security = Security::new(&sid, true)?;
        let attributes = security.attributes();
        let name = wide(path.as_os_str());
        if let Err(cause) = unsafe { CreateDirectoryW(PCWSTR(name.as_ptr()), Some(&attributes)) } {
            if !path.try_exists()? {
                return Err(win(cause));
            }
        }
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_attributes() & 0x400 != 0 {
        return Err(Error::Unavailable(
            "Local owner directory is not a regular directory".into(),
        ));
    }
    let name = wide(path.as_os_str());
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    let mut owner = PSID::default();
    let mut acl: *mut ACL = std::ptr::null_mut();
    let result = unsafe {
        GetNamedSecurityInfoW(
            PCWSTR(name.as_ptr()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            Some(&mut acl),
            None,
            &mut descriptor,
        )
    };
    if result.0 != 0 {
        return Err(std::io::Error::from_raw_os_error(result.0 as i32).into());
    }
    let _allocation = Allocation(descriptor.0);
    if sid_string(owner)? != sid {
        return Err(Error::Unavailable(
            "Local owner directory belongs to another user".into(),
        ));
    }
    unsafe {
        if acl.is_null() || !IsValidAcl(acl).as_bool() || (*acl).AceCount != 1 {
            return Err(Error::Unavailable(
                "Local owner directory is not restricted to the current user".into(),
            ));
        }
        let mut entry = std::ptr::null_mut();
        GetAce(acl, 0, &mut entry).map_err(win)?;
        let ace = &*entry.cast::<ACCESS_ALLOWED_ACE>();
        if ace.Header.AceType != 0
            || usize::from(ace.Header.AceSize) < std::mem::size_of::<ACCESS_ALLOWED_ACE>()
            || sid_string(PSID(std::ptr::addr_of!(ace.SidStart).cast_mut().cast()))? != sid
        {
            return Err(Error::Unavailable(
                "Local owner directory grants another user access".into(),
            ));
        }
    }
    Ok(())
}

pub(super) fn address(_root: &Path, key: &str) -> Result<Address> {
    Ok(format!(
        r"\\.\pipe\loofah-owner-{}-{}",
        &super::hash(current_sid()?.as_bytes())[..16],
        key
    ))
}

fn create(name: &str, sid: &str, first: bool) -> Result<NamedPipeServer> {
    let security = Security::new(sid, false)?;
    let mut attributes = security.attributes();
    // The protected DACL names only this kernel-verified user SID. Windows copies it at creation.
    unsafe {
        ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .max_instances(8)
            .create_with_security_attributes_raw(
                name,
                (&mut attributes as *mut SECURITY_ATTRIBUTES).cast(),
            )
            .map_err(Error::from)
    }
}

pub(super) async fn bind(name: &Address) -> Result<Listener> {
    let sid = current_sid()?;
    let pending = create(name, &sid, true)?;
    Ok(Listener {
        pending: Some(pending),
        name: name.clone(),
        sid,
    })
}
impl Listener {
    pub(crate) async fn accept(&mut self) -> Result<Stream> {
        let pending = self
            .pending
            .as_ref()
            .ok_or_else(|| Error::protocol("Missing local pipe listener"))?;
        pending.connect().await?;
        let connected = self.pending.take().unwrap();
        // Keep an instance alive while installing the next listener; no pipe-name creation gap.
        self.pending = Some(create(&self.name, &self.sid, false)?);
        let mut pid = 0;
        unsafe {
            GetNamedPipeClientProcessId(HANDLE(connected.as_raw_handle()), &mut pid)
                .map_err(win)?;
        }
        verify_process(pid, &self.sid)?;
        Ok(Box::new(connected))
    }
}
fn verify_process(pid: u32, expected: &str) -> Result<()> {
    let process =
        Handle(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).map_err(win)? });
    if process_sid(process.0)? != expected {
        return Err(Error::Unavailable(
            "Local owner peer is not the current user".into(),
        ));
    }
    Ok(())
}

pub(super) async fn connect(name: &Address) -> Result<Stream> {
    let client = ClientOptions::new().open(name)?;
    let mut pid = 0;
    unsafe {
        GetNamedPipeServerProcessId(HANDLE(client.as_raw_handle()), &mut pid).map_err(win)?;
    }
    verify_process(pid, &current_sid()?)?;
    Ok(Box::new(client))
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Security::{
        GetSecurityDescriptorControl, GetSecurityDescriptorDacl, SE_DACL_PROTECTED,
    };

    #[test]
    fn security_descriptor_is_protected_and_grants_only_the_kernel_user_sid() {
        let sid = current_sid().unwrap();
        let security = Security::new(&sid, false).unwrap();
        unsafe {
            let mut control = 0;
            let mut revision = 0;
            GetSecurityDescriptorControl(security.descriptor, &mut control, &mut revision).unwrap();
            assert_ne!(control & SE_DACL_PROTECTED.0, 0);
            let mut present = false.into();
            let mut defaulted = false.into();
            let mut acl = std::ptr::null_mut();
            GetSecurityDescriptorDacl(security.descriptor, &mut present, &mut acl, &mut defaulted)
                .unwrap();
            assert!(present.as_bool());
            assert!(!acl.is_null());
            assert_eq!((*acl).AceCount, 1);
            let mut entry = std::ptr::null_mut();
            GetAce(acl, 0, &mut entry).unwrap();
            let ace = &*entry.cast::<ACCESS_ALLOWED_ACE>();
            assert_eq!(ace.Header.AceType, 0);
            assert_eq!(
                sid_string(PSID(std::ptr::addr_of!(ace.SidStart).cast_mut().cast())).unwrap(),
                sid
            );
        }
        assert!(verify_process(std::process::id(), &sid).is_ok());
        assert!(verify_process(std::process::id(), "S-1-5-18-unrelated").is_err());
    }
}
