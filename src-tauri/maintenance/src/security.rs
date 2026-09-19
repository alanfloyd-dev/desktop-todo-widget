//! Explicit current-user ACL for named coordination objects.
use crate::{paths, Result};
use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::*,
        Security::{Authorization::*, *},
        System::Threading::*,
    },
};
pub struct Descriptor(pub PSECURITY_DESCRIPTOR);
impl Descriptor {
    pub fn current_user() -> Result<Self> {
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
            let result = (|| {
                let mut size = 0;
                let _ = GetTokenInformation(token, TokenUser, None, 0, &mut size);
                let mut data = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
                GetTokenInformation(
                    token,
                    TokenUser,
                    Some(data.as_mut_ptr().cast()),
                    size,
                    &mut size,
                )?;
                let user = &*data.as_ptr().cast::<TOKEN_USER>();
                let mut sid = PWSTR::null();
                ConvertSidToStringSidW(user.User.Sid, &mut sid)?;
                let sid_text = sid.to_string();
                let _ = LocalFree(Some(HLOCAL(sid.0.cast())));
                let sid_text = sid_text
                    .map_err(|e| crate::Error::new(crate::ErrorKind::UnsafePath, e.to_string()))?;
                let sddl = paths::wide(format!("D:P(A;;GA;;;SY)(A;;GA;;;{sid_text})"));
                let mut descriptor = PSECURITY_DESCRIPTOR::default();
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    PCWSTR(sddl.as_ptr()),
                    1,
                    &mut descriptor,
                    None,
                )?;
                Ok(Self(descriptor))
            })();
            let _ = CloseHandle(token);
            result
        }
    }
    pub fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0 .0,
            bInheritHandle: false.into(),
        }
    }
}
impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.0 .0)));
        }
    }
}
