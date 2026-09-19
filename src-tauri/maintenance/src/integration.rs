use crate::{
    paths::{self, Paths},
    receipt::{Integration, Receipt},
    Error, ErrorKind, Result, HELPER_EXE, MAIN_EXE,
};
use std::collections::BTreeMap;
use windows::{
    core::{Interface, PCWSTR},
    Win32::{
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED, STGM_READ,
        },
        UI::Shell::{IShellLinkW, ShellLink},
    },
};
use winreg::{enums::*, RegKey};

pub fn registration(paths: &Paths, receipt: &Receipt) -> BTreeMap<&'static str, String> {
    BTreeMap::from([
        ("DisplayName", "desktop-todo-widget".into()),
        (
            "DisplayVersion",
            receipt.current_version.clone().unwrap_or_default(),
        ),
        ("Publisher", "Alan Floyd".into()),
        (
            "DisplayIcon",
            format!("\"{}\",0", paths.install().join(MAIN_EXE).display()),
        ),
        ("InstallLocation", paths.install().display().to_string()),
        (
            "UninstallString",
            format!(
                "\"{}\" --uninstall",
                paths.install().join(HELPER_EXE).display()
            ),
        ),
        ("InstallationId", receipt.installation_id.clone()),
    ])
}
fn existing(paths: &Paths) -> Result<Option<RegKey>> {
    match RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(paths.registry(), KEY_READ | KEY_WRITE | KEY_WOW64_64KEY)
    {
        Ok(key) => Ok(Some(key)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn owned(paths: &Paths, key: &RegKey) -> bool {
    let location: std::io::Result<String> = key.get_value("InstallLocation");
    let command: std::io::Result<String> = key.get_value("UninstallString");
    location.is_ok_and(|p| paths::equal(std::path::Path::new(&p), paths.install()))
        && command.is_ok_and(|s| {
            s == format!(
                "\"{}\" --uninstall",
                paths.install().join(HELPER_EXE).display()
            )
        })
}
pub fn registration_owned(paths: &Paths) -> Result<bool> {
    Ok(existing(paths)?.is_some_and(|k| owned(paths, &k)))
}
struct Com;
impl Com {
    fn enter() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        }
        Ok(Self)
    }
}
impl Drop for Com {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
fn shortcut_target(paths: &Paths) -> Result<Option<std::path::PathBuf>> {
    if !paths.shortcut().exists() {
        return Ok(None);
    }
    let _parents = paths::pin_parents(paths.shortcut())?;
    let _file = paths::open_regular(paths.shortcut())?;
    let _com = Com::enter()?;
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let persist: IPersistFile = link.cast()?;
        persist.Load(PCWSTR(paths::wide(paths.shortcut()).as_ptr()), STGM_READ)?;
        let mut buffer = [0u16; 32768];
        link.GetPath(&mut buffer, std::ptr::null_mut(), 0)?;
        Ok(Some(std::path::PathBuf::from(String::from_utf16_lossy(
            &buffer[..buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len())],
        ))))
    }
}
pub fn reconcile(paths: &Paths, receipt: &Receipt) -> Result<()> {
    receipt.validate(paths)?;
    let result = (|| {
        if let Some(key) = existing(paths)? {
            if !owned(paths, &key) {
                return Err(Error::new(
                    ErrorKind::IntegrationFailure,
                    "Uninstall registry identity belongs to another installation",
                ));
            }
        }
        if let Some(target) = shortcut_target(paths)? {
            if !paths::equal(&target, &paths.install().join(MAIN_EXE)) {
                return Err(Error::new(
                    ErrorKind::IntegrationFailure,
                    "Shortcut conflict; preserved",
                ));
            }
        }
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey_with_flags(paths.registry(), KEY_READ | KEY_WRITE | KEY_WOW64_64KEY)?;
        for (name, value) in registration(paths, receipt) {
            key.set_value(name, &value)?;
            if key.get_value::<String, _>(name)? != value {
                return Err(Error::new(
                    ErrorKind::IntegrationFailure,
                    "Registry readback mismatch",
                ));
            }
        }
        for name in ["NoModify", "NoRepair"] {
            key.set_value(name, &1u32)?;
            if key.get_value::<u32, _>(name)? != 1 {
                return Err(Error::new(
                    ErrorKind::IntegrationFailure,
                    "Registry flag mismatch",
                ));
            }
        }
        let want_shortcut = receipt
            .integration_resources
            .iter()
            .any(|r| r.identity == Integration::MainStartMenuShortcut && r.present);
        if want_shortcut {
            let parent = paths
                .shortcut()
                .parent()
                .ok_or_else(|| Error::new(ErrorKind::UnsafePath, "Shortcut parent missing"))?;
            paths::create_dir(parent)?;
            let _parents = paths::pin_parents(paths.shortcut())?;
            let _com = Com::enter()?;
            unsafe {
                let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
                let exe = paths::wide(paths.install().join(MAIN_EXE));
                link.SetPath(PCWSTR(exe.as_ptr()))?;
                link.SetWorkingDirectory(PCWSTR(paths::wide(paths.install()).as_ptr()))?;
                link.SetDescription(windows::core::w!("desktop-todo-widget"))?;
                link.SetIconLocation(PCWSTR(exe.as_ptr()), 0)?;
                let persist: IPersistFile = link.cast()?;
                persist.Save(PCWSTR(paths::wide(paths.shortcut()).as_ptr()), true)?;
            }
        } else {
            remove_shortcut(paths)?;
        }
        if want_shortcut
            && !shortcut_target(paths)?
                .is_some_and(|p| paths::equal(&p, &paths.install().join(MAIN_EXE)))
        {
            return Err(Error::new(
                ErrorKind::IntegrationFailure,
                "Shortcut readback mismatch",
            ));
        }
        Ok(())
    })();
    result.map_err(|e: Error| Error::new(ErrorKind::IntegrationFailure, e.to_string()))
}
pub fn remove_shortcut(paths: &Paths) -> Result<bool> {
    match shortcut_target(paths)? {
        Some(target) if paths::equal(&target, &paths.install().join(MAIN_EXE)) => {
            paths::remove_file(paths.shortcut())
        }
        Some(_) => Ok(false),
        None => Ok(true),
    }
}
/// Only owned values are removed. Unknown values/subkeys are left in place.
pub fn remove(paths: &Paths) -> Result<Vec<String>> {
    let mut residuals = vec![];
    if !remove_shortcut(paths)? {
        residuals.push("Unknown shortcut preserved".into());
    }
    if let Some(key) = existing(paths)? {
        if !owned(paths, &key) {
            residuals.push("Uninstall registration conflict preserved".into());
            return Ok(residuals);
        }
        for name in [
            "DisplayName",
            "DisplayVersion",
            "Publisher",
            "DisplayIcon",
            "InstallLocation",
            "UninstallString",
            "InstallationId",
            "NoModify",
            "NoRepair",
        ] {
            match key.delete_value(name) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        if key.enum_values().next().is_none() && key.enum_keys().next().is_none() {
            drop(key);
            RegKey::predef(HKEY_CURRENT_USER)
                .delete_subkey_with_flags(paths.registry(), KEY_WOW64_64KEY)?;
        } else {
            residuals.push("Unknown registry values/subkeys preserved".into());
        }
    }
    Ok(residuals)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn isolated_registry_shortcut_roundtrip_and_conflict() {
        let id = uuid::Uuid::new_v4().to_string();
        let p = Paths::sandbox(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("target")
                .join("integration-tests")
                .join(&id),
            &id,
        );
        paths::create_dir(p.install()).unwrap();
        let records = [
            crate::resources::Resource::MainExecutable,
            crate::resources::Resource::MaintenanceHelper,
        ]
        .map(|identity| crate::receipt::FileRecord {
            identity,
            version: "1.1.0".into(),
            size: 0,
            sha256: "a".repeat(64),
        })
        .to_vec();
        let r = Receipt::new(&p, "1.1.0", records, true);
        assert_eq!(
            registration(&p, &r)["UninstallString"],
            format!("\"{}\" --uninstall", p.install().join(HELPER_EXE).display())
        );
        reconcile(&p, &r).unwrap();
        reconcile(&p, &r).unwrap();
        assert!(registration_owned(&p).unwrap());
        assert!(p.shortcut().exists());
        remove(&p).unwrap();
        remove(&p).unwrap();
        assert!(!p.shortcut().exists());
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey(p.registry())
            .unwrap();
        key.set_value("InstallLocation", &"C:\\Foreign").unwrap();
        assert!(reconcile(&p, &r).is_err());
        assert!(!remove(&p).unwrap().is_empty());
        key.delete_value("InstallLocation").unwrap();
        drop(key);
        RegKey::predef(HKEY_CURRENT_USER)
            .delete_subkey(p.registry())
            .unwrap();
    }
}
