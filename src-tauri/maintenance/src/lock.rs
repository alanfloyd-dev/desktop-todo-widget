use crate::{
    paths::{self, Paths},
    receipt::{Lifecycle, Receipt},
    Error, ErrorKind, Result,
};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    os::windows::fs::OpenOptionsExt,
};
use windows::{
    core::PCWSTR,
    Win32::{Foundation::*, System::Threading::*},
};

/// Mutex is session-local; share-denied gate file adds cross-session exclusion.
/// Keep both alive for the entire maintenance operation. Neither is a journal.
pub struct Gate {
    mutex: HANDLE,
    _file: File,
}
impl Gate {
    pub fn acquire(paths: &Paths) -> Result<Self> {
        paths::create_dir(paths.state())?;
        let name = paths::wide(format!(
            "Local\\desktop-todo-maintenance-{:x}",
            Sha256::digest(paths.install().to_string_lossy().to_lowercase().as_bytes())
        ));
        let descriptor = crate::security::Descriptor::current_user()?;
        let attributes = descriptor.attributes();
        let mutex = unsafe { CreateMutexW(Some(&attributes), false, PCWSTR(name.as_ptr()))? };
        let outcome = unsafe { WaitForSingleObject(mutex, 0) };
        if outcome != WAIT_OBJECT_0 && outcome != WAIT_ABANDONED {
            unsafe {
                let _ = CloseHandle(mutex);
            }
            return Err(Error::new(
                ErrorKind::LockUnavailable,
                "Another maintenance operation is active",
            ));
        }
        let path = paths.state().join("admission.lock");
        let result = (|| {
            let _parents = paths::pin_parents(&path)?;
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .share_mode(0)
                .custom_flags(0x00200000)
                .open(&path)?;
            paths::check_handle(&file, false)?;
            Ok(Self { mutex, _file: file })
        })();
        if result.is_err() {
            unsafe {
                let _ = ReleaseMutex(mutex);
                let _ = CloseHandle(mutex);
            }
        }
        result.map_err(|e: Error| Error::new(ErrorKind::LockUnavailable, e.to_string()))
    }
}
impl Drop for Gate {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.mutex);
            let _ = CloseHandle(self.mutex);
        }
    }
}

/// Every application instance retains a shared lease until its process exits.
/// Maintenance needs an exclusive lease, so it cannot delete a live DB/runtime.
pub struct AppLease {
    _file: File,
}
fn lease_file(paths: &Paths, exclusive: bool) -> Result<File> {
    let path = paths.state().join("application.lock");
    let _parents = paths::pin_parents(&path)?;
    if !path.exists() {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .share_mode(if exclusive { 0 } else { 1 })
        .custom_flags(0x00200000)
        .open(&path)?;
    paths::check_handle(&file, false)?;
    Ok(file)
}
pub fn enter_app(paths: &Paths) -> Result<AppLease> {
    let _gate = Gate::acquire(paths)?;
    if paths.state().join("active-uninstall.json").exists() {
        return Err(Error::new(
            ErrorKind::InvalidInstallation,
            "Uninstall is incomplete. Run the maintenance helper again.",
        ));
    }
    if let Some(receipt) = Receipt::load(paths)? {
        if receipt.lifecycle_state != Lifecycle::Installed {
            return Err(Error::new(ErrorKind::InvalidInstallation, "Installation needs maintenance. Run the original trusted package or uninstall helper."));
        }
    } else if paths.install().join(crate::HELPER_EXE).exists() {
        // Missing receipt cannot make a managed installation launch during recovery.
        return Err(Error::new(
            ErrorKind::InvalidInstallation,
            "Managed receipt missing; run the trusted installer or uninstall helper.",
        ));
    }
    Ok(AppLease {
        _file: lease_file(paths, false)?,
    })
}
pub fn exclusive_app(paths: &Paths) -> Result<AppLease> {
    lease_file(paths, true)
        .map(|file| AppLease { _file: file })
        .map_err(|e| {
            Error::new(
                ErrorKind::MainProcessStillRunning,
                format!("Quit all desktop-todo-widget instances from the tray and retry. {e}"),
            )
        })
}
/// Shared lease without admission checks, for callers that already classified
/// the durable maintenance state themselves (the main application's launch
/// admission). Held until process exit so maintenance cannot start mid-session.
pub fn shared_app(paths: &Paths) -> Result<AppLease> {
    lease_file(paths, false).map(|_file| AppLease { _file })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_app_leases_block_maintenance_and_crash_marker_blocks_launch() {
        let id = uuid::Uuid::new_v4().to_string();
        let p = Paths::sandbox(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("target")
                .join("lock-tests")
                .join(&id),
            &id,
        );
        let first = enter_app(&p).unwrap();
        let second = enter_app(&p).unwrap();
        assert!(exclusive_app(&p).is_err());
        drop(first);
        assert!(exclusive_app(&p).is_err());
        drop(second);
        let gate = Gate::acquire(&p).unwrap();
        assert!(Gate::acquire(&p).is_err());
        let exclusive = exclusive_app(&p).unwrap();
        assert!(enter_app(&p).is_err());
        drop(exclusive);
        drop(gate);
        std::fs::write(p.state().join("active-uninstall.json"), "{}").unwrap();
        assert!(enter_app(&p).is_err());
    }
}
