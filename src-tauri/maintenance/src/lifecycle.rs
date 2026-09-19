use crate::{
    integration,
    lock::{self, Gate},
    paths::{self, Paths},
    receipt::{self, FileRecord, Lifecycle, Receipt},
    resources::{self, Resource},
    Error, ErrorKind, Result, APP_ID, HELPER_EXE, MAIN_EXE, PRODUCT_VERSION,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
};
use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::*,
        Storage::FileSystem::*,
        System::{Diagnostics::ToolHelp::*, Threading::*},
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataMode {
    KeepUserData,
    RemoveUserData,
}
#[derive(Debug, Default)]
pub struct Outcome {
    pub preserved: Vec<String>,
}

/// PE metadata is local identity evidence, not publisher authentication.
pub fn product_version(path: &Path) -> Result<String> {
    let _parents = paths::pin_parents(path)?;
    let _file = paths::open_regular(path)?;
    unsafe {
        let name = paths::wide(path);
        let size = GetFileVersionInfoSizeW(PCWSTR(name.as_ptr()), None);
        if size == 0 || size > 1024 * 1024 {
            return Err(Error::new(
                ErrorKind::InvalidInstallation,
                "Missing or oversized PE version information",
            ));
        }
        let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
        GetFileVersionInfoW(
            PCWSTR(name.as_ptr()),
            None,
            size,
            buffer.as_mut_ptr().cast(),
        )?;
        let mut value = std::ptr::null_mut();
        let mut len = 0;
        if !VerQueryValueW(
            buffer.as_ptr().cast(),
            windows::core::w!("\\"),
            &mut value,
            &mut len,
        )
        .as_bool()
            || len < std::mem::size_of::<VS_FIXEDFILEINFO>() as u32
        {
            return Err(Error::new(
                ErrorKind::InvalidInstallation,
                "Invalid PE version resource",
            ));
        }
        let info = std::ptr::read_unaligned(value.cast::<VS_FIXEDFILEINFO>());
        if info.dwSignature != 0xfeef04bd || info.dwProductVersionLS & 0xffff != 0 {
            return Err(Error::new(
                ErrorKind::InvalidInstallation,
                "Unsupported PE version",
            ));
        }
        if !VerQueryValueW(
            buffer.as_ptr().cast(),
            windows::core::w!("\\VarFileInfo\\Translation"),
            &mut value,
            &mut len,
        )
        .as_bool()
            || len < 4
        {
            return Err(Error::new(
                ErrorKind::InvalidInstallation,
                "Missing PE product identity",
            ));
        }
        let language = std::ptr::read_unaligned(value.cast::<u16>());
        let codepage = std::ptr::read_unaligned(value.cast::<u16>().add(1));
        let query = paths::wide(format!(
            "\\StringFileInfo\\{language:04x}{codepage:04x}\\ProductName"
        ));
        if !VerQueryValueW(
            buffer.as_ptr().cast(),
            PCWSTR(query.as_ptr()),
            &mut value,
            &mut len,
        )
        .as_bool()
            || len == 0
        {
            return Err(Error::new(
                ErrorKind::InvalidInstallation,
                "Missing PE ProductName",
            ));
        }
        let name = String::from_utf16_lossy(std::slice::from_raw_parts(
            value.cast::<u16>(),
            len.saturating_sub(1) as usize,
        ));
        if name != "desktop-todo-widget" {
            return Err(Error::new(
                ErrorKind::InvalidInstallation,
                "Unexpected PE ProductName",
            ));
        }
        Ok(format!(
            "{}.{}.{}",
            info.dwProductVersionMS >> 16,
            info.dwProductVersionMS & 0xffff,
            info.dwProductVersionLS >> 16
        ))
    }
}

/// The current app holds a lease, older app builds do not. Refuse any live legacy
/// product image (even a portable copy, because it shares the business DB).
pub fn require_no_legacy_processes(paths: &Paths) -> Result<()> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?;
        let result = (|| {
            let mut entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            let mut next = Process32FirstW(snapshot, &mut entry);
            while next.is_ok() {
                let name = String::from_utf16_lossy(
                    &entry.szExeFile[..entry
                        .szExeFile
                        .iter()
                        .position(|c| *c == 0)
                        .unwrap_or(entry.szExeFile.len())],
                );
                if [MAIN_EXE, "alan-desktop.exe"]
                    .iter()
                    .any(|n| name.eq_ignore_ascii_case(n))
                {
                    // No termination by PID/name. Inaccessible identity also fails closed.
                    let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, entry.th32ProcessID).map_err(|_| Error::new(ErrorKind::MainProcessStillRunning, "A product-named process cannot be inspected; close it before maintenance"))?;
                    let mut image = vec![0u16; 32768];
                    let mut len = image.len() as u32;
                    let query = QueryFullProcessImageNameW(
                        handle,
                        PROCESS_NAME_WIN32,
                        PWSTR(image.as_mut_ptr()),
                        &mut len,
                    );
                    let _ = CloseHandle(handle);
                    query?;
                    if paths.is_qa()
                        && !paths::equal(
                            Path::new(&String::from_utf16_lossy(&image[..len as usize])),
                            &paths.install().join(MAIN_EXE),
                        )
                    {
                        next = Process32NextW(snapshot, &mut entry);
                        continue;
                    }
                    return Err(Error::new(
                        ErrorKind::MainProcessStillRunning,
                        "Quit all desktop-todo-widget instances from their tray menus, then retry",
                    ));
                }
                next = Process32NextW(snapshot, &mut entry);
            }
            Ok(())
        })();
        let _ = CloseHandle(snapshot);
        result
    }
}

fn log(paths: &Paths, phase: &str, detail: &str) -> Result<()> {
    let path = paths.state().join("maintenance.log");
    let _parents = paths::pin_parents(&path)?;
    if path.exists() {
        drop(paths::open_regular(&path)?);
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    paths::check_handle(&f, false)?;
    writeln!(
        f,
        "{} phase={} {}",
        chrono::Utc::now().to_rfc3339(),
        phase,
        detail
    )?;
    f.sync_all()?;
    Ok(())
}

fn record(source: &Path, identity: Resource, version: &str) -> Result<FileRecord> {
    let (size, sha256) = receipt::fingerprint(&source.join(identity.filename()))?;
    Ok(FileRecord {
        identity,
        version: version.into(),
        size,
        sha256,
    })
}
fn copy_verified(source: &Path, destination: &Path, expected: &FileRecord) -> Result<()> {
    let _source_parents = paths::pin_parents(source)?;
    let _target_parents = paths::pin_parents(destination)?;
    let mut input = paths::open_regular(source)?;
    let temp = destination.with_extension(format!("{}.installing", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        std::io::copy(&mut input, &mut output)?;
        output.sync_all()?;
        drop(output);
        if receipt::fingerprint(&temp)? != (expected.size, expected.sha256.clone()) {
            return Err(Error::new(
                ErrorKind::InvalidInstallation,
                "Payload changed while copying",
            ));
        }
        if destination.exists() {
            drop(paths::open_regular(destination)?);
        }
        paths::move_replace(&temp, destination)
    })();
    if result.is_err() {
        let _ = paths::remove_file(&temp);
    }
    result
}

/// True when semantic version `a` is strictly newer than `b`. A parse failure
/// reports "newer" so an unparseable comparison refuses the operation; in
/// practice `Receipt::validate` rejects malformed versions before this runs.
fn version_greater(a: &str, b: &str) -> bool {
    let parse = |v: &str| -> Option<(u32, u32, u32)> {
        let parts: Vec<&str> = v.split('.').collect();
        if parts.len() != 3 {
            return None;
        }
        Some((
            parts[0].parse().ok()?,
            parts[1].parse().ok()?,
            parts[2].parse().ok()?,
        ))
    };
    match (parse(a), parse(b)) {
        (Some(a), Some(b)) => a > b,
        _ => true,
    }
}

/// Manual trusted bootstrap only. Network updates are deliberately absent.
pub fn install(paths: &Paths, source: &Path, shortcut: bool) -> Result<Receipt> {
    paths.validate()?;
    if paths::equal(source, paths.install()) {
        return Err(Error::new(
            ErrorKind::InvalidInstallation,
            "Run install from the extracted trusted package, not the installed directory",
        ));
    }
    for name in [MAIN_EXE, HELPER_EXE] {
        if product_version(&source.join(name))? != PRODUCT_VERSION {
            return Err(Error::new(
                ErrorKind::InvalidInstallation,
                "Package and helper ProductVersion differ",
            ));
        }
    }
    let _gate = Gate::acquire(paths)?;
    let _app = lock::exclusive_app(paths)?;
    require_no_legacy_processes(paths)?;
    if paths.state().join("active-uninstall.json").exists() {
        return Err(Error::new(
            ErrorKind::InvalidInstallation,
            "Resume incomplete uninstall first",
        ));
    }
    let old = Receipt::load(paths)?;
    if let Some(version) = old.as_ref().and_then(|r| r.current_version.as_deref()) {
        // A manual bootstrap may reinstall the same release or install a newer
        // one; downgrades are refused (uninstall first).
        if version_greater(version, PRODUCT_VERSION) {
            return Err(Error::new(
                ErrorKind::InvalidInstallation,
                format!(
                    "Downgrade refused: the installation has {version}, this installer is {PRODUCT_VERSION}"
                ),
            ));
        }
    }
    if old.as_ref().is_some_and(|r| {
        !matches!(
            r.lifecycle_state,
            Lifecycle::Installed | Lifecycle::Installing
        )
    }) {
        return Err(Error::new(
            ErrorKind::InvalidInstallation,
            "Installation requires recovery/uninstall",
        ));
    }
    // An Installing receipt is the durable marker of an interrupted bootstrap;
    // rerunning the same trusted payload is the documented resume path. The
    // interrupted transaction only ever declared the compiled runtime paths,
    // so a verified product runtime left at the main executable path by the
    // crash (a v1.1 install it interrupted, or same-version bytes) is a
    // legitimate pre-image. Every other on-disk state still refuses below.
    let retrying = old
        .as_ref()
        .is_some_and(|r| r.lifecycle_state == Lifecycle::Installing);
    let files = [Resource::MainExecutable, Resource::MaintenanceHelper]
        .into_iter()
        .map(|id| record(source, id, PRODUCT_VERSION))
        .collect::<Result<Vec<_>>>()?;
    // Existing managed content must match either the old record or this exact
    // manual retry's payload. Foreign binaries are not silently overwritten.
    for file in &files {
        let target = paths.install().join(file.identity.filename());
        if target.exists() {
            let actual = receipt::fingerprint(&target)?;
            let old_match = old.as_ref().is_some_and(|r| {
                r.runtime_resources
                    .iter()
                    .any(|f| f.identity == file.identity && (f.size, f.sha256.clone()) == actual)
            });
            let new_match = actual == (file.size, file.sha256.clone());
            let legacy = (old.is_none() || retrying)
                && file.identity == Resource::MainExecutable
                && product_version(&target).is_ok_and(|v| v == "1.1.0" || v == PRODUCT_VERSION);
            if !old_match && !new_match && !legacy {
                return Err(Error::new(
                    ErrorKind::InvalidInstallation,
                    "Existing runtime ownership/hash conflict",
                ));
            }
        }
    }
    paths::create_dir(paths.install())?;
    let mut receipt = Receipt::new(paths, PRODUCT_VERSION, files, shortcut);
    if let Some(previous) = old {
        receipt.installation_id = previous.installation_id;
        receipt.installed_at = previous.installed_at;
        receipt.maintenance.receipt_generation = previous.maintenance.receipt_generation;
        // Reinstalls keep support-file ownership that is still verifiable on
        // disk. A record whose file is gone, or no longer matches its recorded
        // fingerprint, is dropped: the file becomes unowned and uninstall
        // preserves it as unknown. Identities are bounded by the compiled
        // resource policy — deserialization already rejects anything else.
        receipt.support_resources = previous
            .support_resources
            .into_iter()
            .filter(|record| {
                let target = paths.install().join(record.identity.filename());
                target.exists()
                    && receipt::fingerprint(&target)
                        .is_ok_and(|actual| actual == (record.size, record.sha256.clone()))
            })
            .collect();
    }
    receipt.save(paths)?;
    log(
        paths,
        "Installing",
        &format!(
            "installationId={} version={} install={} data={}",
            receipt.installation_id,
            PRODUCT_VERSION,
            paths.install().display(),
            paths.data().display()
        ),
    )?;
    for file in &receipt.runtime_resources {
        copy_verified(
            &source.join(file.identity.filename()),
            &paths.install().join(file.identity.filename()),
            file,
        )?;
    }
    for id in [
        Resource::Readme,
        Resource::ReadmeZh,
        Resource::License,
        Resource::LicenseZh,
        Resource::ThirdPartyNotices,
    ] {
        let source_file = source.join(id.filename());
        let target = paths.install().join(id.filename());
        if source_file.exists() && !target.exists() {
            let file = record(source, id, PRODUCT_VERSION)?;
            copy_verified(&source_file, &target, &file)?;
            receipt.support_resources.push(file);
        }
    }
    for name in [MAIN_EXE, HELPER_EXE] {
        if product_version(&paths.install().join(name))? != PRODUCT_VERSION {
            return Err(Error::new(
                ErrorKind::InvalidInstallation,
                "Installed version mismatch",
            ));
        }
    }
    integration::reconcile(paths, &receipt)?;
    receipt.lifecycle_state = Lifecycle::Installed;
    receipt.save(paths)?;
    log(
        paths,
        "Installed",
        "runtime verified; receipt and Windows integration reconciled",
    )?;
    Ok(receipt)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UninstallJournal {
    schema_version: u32,
    app_id: String,
    installation_id: Option<String>,
    install_root: std::path::PathBuf,
    phase: String,
}
fn phase(paths: &Paths, receipt: Option<&Receipt>, name: &str) -> Result<()> {
    let journal = UninstallJournal {
        schema_version: 1,
        app_id: APP_ID.into(),
        installation_id: receipt.map(|r| r.installation_id.clone()),
        install_root: paths.install().into(),
        phase: name.into(),
    };
    let bytes = serde_json::to_vec_pretty(&journal)
        .map_err(|e| Error::new(ErrorKind::PartialUninstall, e.to_string()))?;
    paths::atomic_write(&paths.state().join("active-uninstall.json"), &bytes)?;
    log(paths, name, "operation=uninstall")
}
fn validate_journal(paths: &Paths) -> Result<()> {
    let path = paths.state().join("active-uninstall.json");
    if !path.exists() {
        return Ok(());
    }
    let mut bytes = Vec::new();
    paths::open_regular(&path)?
        .take(262145)
        .read_to_end(&mut bytes)?;
    let j: UninstallJournal = serde_json::from_slice(&bytes)
        .map_err(|e| Error::new(ErrorKind::PartialUninstall, e.to_string()))?;
    if bytes.len() > 262144
        || j.schema_version != 1
        || j.app_id != APP_ID
        || !paths::equal(&j.install_root, paths.install())
    {
        return Err(Error::new(
            ErrorKind::InvalidInstallation,
            "Uninstall journal mismatch",
        ));
    }
    if let Some(id) = j.installation_id {
        receipt::validate_uuid(&id)?;
    }
    Ok(())
}
pub fn can_remove_data(paths: &Paths) -> bool {
    matches!(Receipt::load(paths), Ok(Some(_)))
}

/// Called only after native confirmation, while caller holds the gate.
pub fn uninstall_locked(paths: &Paths, mode: DataMode) -> Result<Outcome> {
    paths.validate()?;
    validate_journal(paths)?;
    let _app = lock::exclusive_app(paths)?;
    require_no_legacy_processes(paths)?;
    let mut receipt = match Receipt::load(paths) {
        Ok(r) => r,
        Err(e)
            if matches!(
                e.kind,
                ErrorKind::ReceiptMalformed
                    | ErrorKind::InvalidInstallation
                    | ErrorKind::AppIdMismatch
                    | ErrorKind::InstallRootMismatch
                    | ErrorKind::DataRootMismatch
            ) =>
        {
            log(paths, "ConservativeDiscovery", &e.to_string())?;
            None
        }
        Err(e) => return Err(e),
    };
    if receipt.is_none() && mode == DataMode::RemoveUserData {
        return Err(Error::new(
            ErrorKind::InvalidInstallation,
            "Valid receipt required for data deletion; local data kept",
        ));
    }
    if receipt.is_none()
        && !paths.install().join(MAIN_EXE).exists()
        && !paths.install().join(HELPER_EXE).exists()
        && !paths.receipt().exists()
        && !paths.state().join("active-uninstall.json").exists()
        && !integration::registration_owned(paths)?
    {
        return Ok(Outcome::default());
    }
    if receipt.is_none()
        && !integration::registration_owned(paths)?
        && !paths.state().join("active-uninstall.json").exists()
    {
        return Err(Error::new(
            ErrorKind::InvalidInstallation,
            "No corroborated managed installation; no files deleted",
        ));
    }
    if let Some(r) = &mut receipt {
        if matches!(
            r.lifecycle_state,
            Lifecycle::Updating | Lifecycle::RecoveryRequired
        ) {
            return Err(Error::new(
                ErrorKind::InvalidInstallation,
                "Unsupported recovery state; preserved",
            ));
        }
        r.lifecycle_state = Lifecycle::Uninstalling;
        r.save(paths)?;
        log(
            paths,
            "LoadingInstallation",
            &format!(
                "installationId={} version={:?} install={} data={}",
                r.installation_id,
                r.current_version,
                paths.install().display(),
                paths.data().display()
            ),
        )?;
    }
    phase(paths, receipt.as_ref(), "RemovingIntegration")?;
    let mut result = Outcome {
        preserved: integration::remove(paths)?,
    };
    phase(paths, receipt.as_ref(), "RemovingRuntimeFiles")?;
    for identity in [Resource::MainExecutable, Resource::MaintenanceHelper] {
        let path = paths.install().join(identity.filename());
        if path.exists() {
            if let Some(r) = &receipt {
                let expected = r
                    .runtime_resources
                    .iter()
                    .find(|f| f.identity == identity)
                    .ok_or_else(|| {
                        Error::new(ErrorKind::InvalidInstallation, "Missing runtime record")
                    })?;
                if receipt::fingerprint(&path)? != (expected.size, expected.sha256.clone()) {
                    return Err(Error::new(
                        ErrorKind::RuntimeRemovalFailure,
                        "Runtime changed; preserved for manual repair",
                    ));
                }
            } else {
                product_version(&path)?;
            }
        }
        let removed = paths::remove_file(&path)
            .map_err(|e| Error::new(ErrorKind::RuntimeRemovalFailure, e.to_string()))?;
        log(
            paths,
            "RemovingRuntimeFiles",
            &format!("resource={identity:?} removed={removed}"),
        )?;
    }
    if let Some(r) = &receipt {
        for f in &r.support_resources {
            let path = paths.install().join(f.identity.filename());
            if path.exists() && receipt::fingerprint(&path)? == (f.size, f.sha256.clone()) {
                paths::remove_file(&path)?;
            } else if path.exists() {
                result
                    .preserved
                    .push(format!("Modified document {}", f.identity.filename()));
            }
        }
    }
    phase(paths, receipt.as_ref(), "RemovingEphemeralState")?;
    paths::remove_file(&paths.install().join("qa-diagnostics.log"))?;
    if mode == DataMode::RemoveUserData {
        phase(paths, receipt.as_ref(), "OptionallyRemovingUserData")?;
        remove_data(paths, &mut result)
            .map_err(|e| Error::new(ErrorKind::PersistentDataRemovalFailure, e.to_string()))?;
    }
    phase(paths, receipt.as_ref(), "Finalizing")?;
    if receipt.is_some() {
        paths::remove_file(&paths.receipt())?;
    }
    if !paths::remove_empty_dir(paths.install())? {
        result
            .preserved
            .push("Unknown files remain in installation directory".into());
    }
    log(
        paths,
        "Completed",
        &format!("mode={mode:?}; preserved={:?}", result.preserved),
    )?;
    paths::remove_file(&paths.state().join("active-uninstall.json"))?;
    Ok(result)
}
fn remove_data(paths: &Paths, outcome: &mut Outcome) -> Result<()> {
    if !paths.data().exists() {
        return Ok(());
    }
    let _parents = paths::pin_parents(&paths.data().join("entry"))?;
    for entry in fs::read_dir(paths.data())? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if resources::data_file(&name) {
            paths::remove_file(&entry.path())?;
        } else if name == "assets" {
            let _asset_parents = paths::pin_parents(&entry.path().join("entry"))?;
            for asset in fs::read_dir(entry.path())? {
                let asset = asset?;
                if resources::asset_file(&asset.file_name().to_string_lossy()) {
                    paths::remove_file(&asset.path())?;
                } else {
                    outcome.preserved.push("Unknown asset preserved".into());
                }
            }
            drop(_asset_parents);
            paths::remove_empty_dir(&entry.path())?;
        } else {
            outcome
                .preserved
                .push("Unknown data resource preserved".into());
        }
    }
    drop(_parents);
    paths::remove_empty_dir(paths.data())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn version_ordering_is_semantic_and_fails_closed() {
        assert!(version_greater("1.3.0", "1.2.9"));
        assert!(version_greater("1.10.0", "1.9.0"));
        assert!(version_greater("2.0.0", "1.99.99"));
        assert!(!version_greater("1.1.0", "1.1.0"));
        assert!(!version_greater("1.0.9", "1.1.0"));
        // Unparseable input must refuse the operation, never silently allow it.
        assert!(version_greater("1.2", "1.1.0"));
        assert!(version_greater("banana", "1.1.0"));
    }
    #[test]
    fn install_rejects_its_own_install_root_as_source() {
        let id = uuid::Uuid::new_v4().to_string();
        let p = Paths::sandbox(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("target")
                .join("install-tests")
                .join(&id),
            &id,
        );
        paths::create_dir(p.install()).unwrap();
        // The source check fires before any payload parsing: installing from
        // the canonical install root itself is always refused.
        let error = install(&p, p.install(), true).unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidInstallation);
        assert!(error.detail.contains("not the installed directory"));
    }
    #[test]
    fn data_cleanup_only_known_names_and_uuid_assets() {
        let id = uuid::Uuid::new_v4().to_string();
        let p = Paths::sandbox(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("target")
                .join("data-tests")
                .join(&id),
            &id,
        );
        paths::create_dir(&p.data().join("assets")).unwrap();
        for file in [
            "alan-desktop.sqlite3",
            "alan-desktop.sqlite3-wal",
            "alan-desktop.sqlite3-shm",
            "sentinel.txt",
        ] {
            fs::write(p.data().join(file), "test").unwrap();
        }
        fs::write(
            p.data().join("assets").join(format!("avatar-{id}.png")),
            "image",
        )
        .unwrap();
        fs::write(p.data().join("assets/foreign.png"), "unknown").unwrap();
        let mut out = Outcome::default();
        remove_data(&p, &mut out).unwrap();
        assert!(!p.data().join("alan-desktop.sqlite3").exists());
        assert!(p.data().join("sentinel.txt").exists());
        assert!(p.data().join("assets/foreign.png").exists());
        remove_data(&p, &mut out).unwrap();
        assert!(!can_remove_data(&p));
    }
    #[test]
    fn uninstall_keep_remove_reinstall_unknown_and_missing_are_safe() {
        let id = uuid::Uuid::new_v4().to_string();
        let p = Paths::sandbox(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("target")
                .join("uninstall-tests")
                .join(&id),
            &id,
        );
        paths::create_dir(p.install()).unwrap();
        paths::create_dir(p.data()).unwrap();
        let _gate = Gate::acquire(&p).unwrap();
        let make_receipt = || {
            for name in [MAIN_EXE, HELPER_EXE] {
                fs::write(p.install().join(name), b"owned runtime fixture").unwrap();
            }
            let files = [Resource::MainExecutable, Resource::MaintenanceHelper]
                .into_iter()
                .map(|r| record(p.install(), r, "1.1.0").unwrap())
                .collect();
            let mut r = Receipt::new(&p, "1.1.0", files, false);
            r.lifecycle_state = Lifecycle::Installed;
            r.save(&p).unwrap();
            r
        };
        let first = make_receipt();
        fs::write(p.install().join("unknown.dll"), "foreign").unwrap();
        fs::write(p.data().join("alan-desktop.sqlite3"), "data").unwrap();
        fs::write(p.data().join("unknown.txt"), "foreign").unwrap();
        paths::remove_file(&p.install().join(MAIN_EXE)).unwrap();
        let out = uninstall_locked(&p, DataMode::KeepUserData).unwrap();
        assert!(!out.preserved.is_empty());
        assert!(p.data().join("alan-desktop.sqlite3").exists());
        assert!(p.install().join("unknown.dll").exists());
        assert!(!p.receipt().exists());
        uninstall_locked(&p, DataMode::KeepUserData).unwrap();
        assert!(uninstall_locked(&p, DataMode::RemoveUserData).is_err());
        let second = make_receipt();
        assert_ne!(first.installation_id, second.installation_id);
        uninstall_locked(&p, DataMode::RemoveUserData).unwrap();
        assert!(!p.data().join("alan-desktop.sqlite3").exists());
        assert!(p.data().join("unknown.txt").exists());
    }
    #[test]
    fn tampered_receipt_never_grants_data_removal() {
        let id = uuid::Uuid::new_v4().to_string();
        let p = Paths::sandbox(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("target")
                .join("tamper-tests")
                .join(&id),
            &id,
        );
        paths::create_dir(p.install()).unwrap();
        paths::create_dir(p.data()).unwrap();
        fs::write(
            p.receipt(),
            br#"{"schemaVersion":1,"installRoot":"C:\\Windows"}"#,
        )
        .unwrap();
        fs::write(p.data().join("alan-desktop.sqlite3"), "safe").unwrap();
        let _gate = Gate::acquire(&p).unwrap();
        assert!(uninstall_locked(&p, DataMode::RemoveUserData).is_err());
        assert!(p.data().join("alan-desktop.sqlite3").exists());
    }
}
