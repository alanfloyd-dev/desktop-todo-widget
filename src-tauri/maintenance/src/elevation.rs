//! Elevation refusal for per-user maintenance.
//!
//! The product is a per-user installation: nothing in install, uninstall, or
//! a future update needs elevation. An elevated process is refused outright
//! because over-the-shoulder elevation (a standard user supplying an
//! administrator's credentials) switches to the administrator's profile,
//! HKCU, and Known Folders — the transaction would then target the wrong
//! user's installation and data. Same-account UAC elevation can keep the
//! same profile, but the product gains nothing from allowing it, so the rule
//! is uniform: per-user maintenance never runs elevated. The check reads the
//! current process token's elevation state directly; it does not inspect
//! group membership, impersonate, resolve an "original" user, or attempt a
//! de-elevated relaunch.

use crate::{Error, ErrorKind, Result};
use windows::{
    Win32::{
        Foundation::{HANDLE, HLOCAL, LocalFree},
        Security::{
            GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
        },
        System::Threading::{GetCurrentProcess, GetCurrentProcessId, OpenProcessToken},
    },
};

/// The decision, factored out so the rule is testable without a token: any
/// nonzero `TokenIsElevated` value means the process token is elevated.
fn token_is_elevated(raw: u32) -> bool {
    raw != 0
}

/// Fails closed when the current process token is elevated. Called before
/// any lifecycle side effect: no gate, no receipt write, no filesystem or
/// registry mutation happens in an elevated helper.
pub fn refuse_elevated_execution() -> Result<()> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(|error| {
            Error::new(
                ErrorKind::ElevatedExecution,
                format!(
                    "Cannot inspect the process token (pid {}): {error}",
                    GetCurrentProcessId()
                ),
            )
        })?;
        let result = (|| {
            let mut returned = 0u32;
            let mut elevation = TOKEN_ELEVATION::default();
            GetTokenInformation(
                token,
                TokenElevation,
                Some(&mut elevation as *mut _ as _),
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut returned,
            )
            .map_err(|error| {
                Error::new(
                    ErrorKind::ElevatedExecution,
                    format!("Cannot query token elevation: {error}"),
                )
            })?;
            if token_is_elevated(elevation.TokenIsElevated) {
                return Err(Error::new(
                    ErrorKind::ElevatedExecution,
                    "This is a per-user application. Run the installer/uninstaller normally, \
                     without administrator elevation.",
                ));
            }
            Ok(())
        })();
        let _ = LocalFree(Some(HLOCAL(token.0.cast())));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::token_is_elevated;

    #[test]
    fn any_nonzero_elevation_state_refuses() {
        assert!(!token_is_elevated(0));
        assert!(token_is_elevated(1));
        assert!(token_is_elevated(0xffff_ffff));
    }
}
