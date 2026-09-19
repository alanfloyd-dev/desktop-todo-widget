use crate::{Error, ErrorKind, Result, APP_ID};
use std::{
    fs::{self, File, OpenOptions},
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::{Component, Path, PathBuf},
};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{ERROR_ACCESS_DENIED, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION, HANDLE},
        Storage::FileSystem::*,
        System::Com::CoTaskMemFree,
        UI::Shell::*,
    },
};

pub fn wide(path: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    path.as_ref().encode_wide().chain(Some(0)).collect()
}
pub fn equal(a: &Path, b: &Path) -> bool {
    a.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
}
fn known(id: &windows::core::GUID) -> Result<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None)?;
        let text = p.to_string();
        CoTaskMemFree(Some(p.0.cast()));
        Ok(PathBuf::from(text.map_err(|e| {
            Error::new(ErrorKind::UnsafePath, e.to_string())
        })?))
    }
}

/// Roots are private: receipt, command line and environment cannot assign them.
#[derive(Debug, Clone)]
pub struct Paths {
    install: PathBuf,
    data: PathBuf,
    state: PathBuf,
    shortcut: PathBuf,
    registry: String,
    qa: bool,
}

/// The one canonical root pair for this product on this Windows user,
/// derived only from Known Folders and compiled policy. Both the maintenance
/// helper and the main application resolve installation identity here;
/// receipts, command lines and environment input cannot assign these paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalRoots {
    pub install: PathBuf,
    pub data: PathBuf,
}
pub fn canonical_roots() -> Result<CanonicalRoots> {
    Ok(CanonicalRoots {
        install: known(&FOLDERID_LocalAppData)?
            .join("Programs")
            .join("desktop-todo-widget"),
        data: known(&FOLDERID_RoamingAppData)?.join(APP_ID),
    })
}
/// The persistent data root (`%APPDATA%\net.alanfloyd.desktop`). The main
/// application resolves its database path from Tauri, which derives the same
/// Known Folder; admission compares receipts against this value.
pub fn canonical_data_root() -> Result<PathBuf> {
    canonical_roots().map(|roots| roots.data)
}
impl Paths {
    pub fn resolve() -> Result<Self> {
        #[cfg(feature = "qa")]
        if let Ok(id) = std::env::var("DTW_MAINTENANCE_QA_ID") {
            crate::receipt::validate_uuid(&id)?;
            let root = known(&FOLDERID_LocalAppData)?
                .join("desktop-todo-maintenance-qa")
                .join(&id);
            return Ok(Self::sandbox(root, &id));
        }
        let canonical = canonical_roots()?;
        Ok(Self {
            install: canonical.install,
            data: canonical.data,
            state: known(&FOLDERID_LocalAppData)?
                .join(APP_ID)
                .join("maintenance"),
            shortcut: known(&FOLDERID_Programs)?.join("desktop-todo-widget.lnk"),
            registry: format!("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{APP_ID}"),
            qa: false,
        })
    }
    #[cfg(any(test, feature = "qa"))]
    pub fn sandbox(root: PathBuf, id: &str) -> Self {
        Self {
            install: root.join("install"),
            data: root.join("data"),
            state: root.join("maintenance"),
            shortcut: root.join("menu").join("desktop-todo-widget.lnk"),
            registry: format!("Software\\desktop-todo-maintenance-qa\\{id}"),
            qa: true,
        }
    }
    pub fn install(&self) -> &Path {
        &self.install
    }
    pub fn data(&self) -> &Path {
        &self.data
    }
    pub fn state(&self) -> &Path {
        &self.state
    }
    pub fn shortcut(&self) -> &Path {
        &self.shortcut
    }
    pub fn registry(&self) -> &str {
        &self.registry
    }
    pub fn is_qa(&self) -> bool {
        self.qa
    }
    pub fn receipt(&self) -> PathBuf {
        self.install.join("installation-receipt.json")
    }
    pub fn validate(&self) -> Result<()> {
        for p in [&self.install, &self.data, &self.state, &self.shortcut] {
            let _ = pin_parents(p)?;
        }
        Ok(())
    }
}

fn safe_syntax(path: &Path) -> Result<()> {
    let text = path.to_string_lossy();
    if !path.is_absolute() || text.starts_with("\\\\") || text.contains('\0') {
        return Err(Error::new(
            ErrorKind::UnsafePath,
            "Only absolute local drive paths are supported",
        ));
    }
    for part in path.components() {
        match part {
            Component::ParentDir | Component::CurDir => {
                return Err(Error::new(ErrorKind::UnsafePath, "Relative path component"))
            }
            Component::Normal(name) => {
                let n = name.to_string_lossy();
                let stem = n.split('.').next().unwrap_or("").to_ascii_uppercase();
                if n.contains([':', '"', '<', '>', '|', '?', '*'])
                    || n.ends_with(['.', ' '])
                    || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                    || (stem.len() == 4
                        && (stem.starts_with("COM") || stem.starts_with("LPT"))
                        && stem.as_bytes()[3].is_ascii_digit())
                {
                    return Err(Error::new(
                        ErrorKind::UnsafePath,
                        "Ambiguous Windows path component",
                    ));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Pins existing ancestors against rename/delete while a path operation executes.
/// OPEN_REPARSE_POINT + handle inspection avoids following a swapped junction.
pub fn pin_parents(path: &Path) -> Result<Vec<File>> {
    safe_syntax(path)?;
    let mut held = Vec::new();
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        if current == path {
            break;
        }
        if !current.is_absolute() {
            continue;
        }
        match OpenOptions::new()
            .access_mode(0)
            .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(&current)
        {
            Ok(file) => {
                check_handle(&file, true)?;
                held.push(file);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => {
                return Err(Error::new(
                    ErrorKind::UnsafePath,
                    format!("Cannot pin {}: {e}", current.display()),
                ))
            }
        }
    }
    Ok(held)
}
pub fn check_handle(file: &File, directory: bool) -> Result<()> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe {
        GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info)?;
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
        || (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0) != directory
        || (!directory && info.nNumberOfLinks != 1)
    {
        return Err(Error::new(
            ErrorKind::UnsafePath,
            "Reparse point, hard link or unexpected file type",
        ));
    }
    Ok(())
}
pub fn open_regular(path: &Path) -> Result<File> {
    let _parents = pin_parents(path)?;
    let f = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)?;
    check_handle(&f, false)?;
    Ok(f)
}
pub fn create_dir(path: &Path) -> Result<()> {
    let _parents = pin_parents(path)?;
    if !path.exists() {
        fs::create_dir_all(path)?;
    }
    let f = OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)?;
    check_handle(&f, true)
}
pub fn remove_file(path: &Path) -> Result<bool> {
    let _parents = pin_parents(path)?;
    let file = match OpenOptions::new()
        .access_mode(DELETE.0 | FILE_READ_ATTRIBUTES.0)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    check_handle(&file, false)?;
    unsafe {
        // POSIX semantics remove the directory entry within this call. The
        // legacy disposition only marks the file delete-pending, leaving the
        // name visible yet temporarily impossible to recreate or replace
        // (ACCESS_DENIED) until every external holder releases it.
        let posix = FILE_DISPOSITION_INFO_EX {
            Flags: FILE_DISPOSITION_INFO_EX_FLAGS(
                FILE_DISPOSITION_FLAG_DELETE.0 | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS.0,
            ),
        };
        let mut done = SetFileInformationByHandle(
            HANDLE(file.as_raw_handle()),
            FileDispositionInfoEx,
            &posix as *const _ as _,
            std::mem::size_of_val(&posix) as u32,
        );
        if done.is_err() {
            // POSIX delete needs Windows 10 1709+; fall back to the
            // delete-pending semantics of older volumes.
            let legacy = FILE_DISPOSITION_INFO {
                DeleteFile: true.into(),
            };
            done = SetFileInformationByHandle(
                HANDLE(file.as_raw_handle()),
                FileDispositionInfo,
                &legacy as *const _ as _,
                std::mem::size_of_val(&legacy) as u32,
            );
        }
        done?;
    }
    Ok(true)
}
pub fn remove_empty_dir(path: &Path) -> Result<bool> {
    let _parents = pin_parents(path)?;
    if !path.exists() {
        return Ok(true);
    }
    if fs::symlink_metadata(path)?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err(Error::new(ErrorKind::UnsafePath, "Linked directory"));
    }
    if fs::read_dir(path)?.next().is_some() {
        return Ok(false);
    }
    fs::remove_dir(path)?;
    Ok(true)
}
/// Replaces `destination` with `source` via MoveFileExW. A replace can
/// transiently fail with ACCESS_DENIED or a sharing violation when an
/// external/system-level sharing conflict is active — real-time scanning,
/// indexing, or the shell touching a file that was just created, renamed or
/// deleted. Probes on real hardware (with a third-party real-time protection
/// service) showed typical denials clearing within ~1 ms; that service can
/// also hold a file for seconds after an Add/Remove-Programs registration
/// change, which no reasonable in-call ladder should ride out. The ladder
/// below therefore covers only the short class (78 ms cap); longer conflicts
/// surface as a precise per-file error, and the manual bootstrap retry is the
/// documented resume path. Every denial observed so far cleared without
/// intervention, which a leaked handle never would, and this crate holds no
/// destination handle across the call, so the bounded retry cannot mask an
/// in-process leak. It never retries other errors.
pub fn move_replace(source: &Path, destination: &Path) -> Result<()> {
    const WAITS_MS: [u64; 6] = [1, 2, 5, 10, 20, 40];
    let mut last = None;
    for attempt in 0..=WAITS_MS.len() {
        let result = unsafe {
            MoveFileExW(
                PCWSTR(wide(source).as_ptr()),
                PCWSTR(wide(destination).as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        match result {
            Ok(()) => return Ok(()),
            Err(e)
                if matches!(
                    (e.code().0 as u32) & 0xffff,
                    x if x == ERROR_ACCESS_DENIED.0
                        || x == ERROR_SHARING_VIOLATION.0
                        || x == ERROR_LOCK_VIOLATION.0
                ) =>
            {
                last = Some(e);
                match WAITS_MS.get(attempt) {
                    Some(wait) => std::thread::sleep(std::time::Duration::from_millis(*wait)),
                    // The ladder is exhausted while the error class is still
                    // exactly a retryable conflict: report the bounded-retry
                    // failure, never fall through to the plain per-file form.
                    None => break,
                }
            }
            Err(e) => {
                return Err(Error::new(
                    ErrorKind::Io,
                    format!(
                        "Atomic replace {}: {e} (win32 code {:#010x})",
                        destination.display(),
                        e.code().0 as u32
                    ),
                ))
            }
        }
    }
    Err(Error::new(
        ErrorKind::Io,
        format!(
            "Atomic replace {}: still denied after {} retries (last win32 code {:#010x}): {}",
            destination.display(),
            WAITS_MS.len(),
            last.as_ref().expect("retry exhausted implies an error").code().0 as u32,
            last.as_ref().expect("retry exhausted implies an error").to_string()
        ),
    ))
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let _parents = pin_parents(path)?;
    if path.exists() {
        drop(open_regular(path)?);
    }
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(0)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        move_replace(&temp, path)
    })();
    if result.is_err() {
        // Do not leave our own unpublished temp behind for the next lifecycle.
        let _ = remove_file(&temp);
    }
    result
}

/// The base name a temp file shares with its compiled-known parent filename:
/// `"installation-receipt.json"` → `"installation-receipt"`, `"LICENSE"` →
/// `"LICENSE"`. The temp primitives replace-or-append the last extension with
/// `.{uuid}.{suffix}`, so this stem is exactly the shared prefix.
pub fn filename_stem(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => name,
    }
}

/// The `<stem>.<uuid>` part of a residue temp name, for names ending in the
/// two suffixes this crate's primitives use. UUID segments contain hyphens
/// but no dots, so the last dot is always the stem boundary. Anything else is
/// not a shape we created.
fn temp_shape(name: &str) -> Option<(&str, &str)> {
    for suffix in [".installing", ".tmp"] {
        if let Some(rest) = name.strip_suffix(suffix) {
            return rest.rsplit_once('.');
        }
    }
    None
}

/// Best-effort cleanup of crash residue left by this crate's own primitives:
/// `atomic_write` publishes via `.{uuid}.tmp` and `copy_verified` via
/// `.{uuid}.installing`, so a process death between creating and publishing
/// either leaves the temp behind. A transaction may remove only temps of the
/// exact shape `<compiled-known stem>.<canonical uuid>.<tmp|installing>` under
/// a canonical root; every other name — unknown files, foreign stems, wrong
/// shapes — is preserved untouched, and cleanup never widens deletion
/// authority beyond what the transaction already owns. Per-file failures are
/// ignored: a locked residue must not block the transaction.
pub fn remove_stale_temps(dir: &Path, stems: &[&str]) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let Some((stem, uuid_segment)) = temp_shape(&name) else {
            continue;
        };
        let known = stems.iter().any(|s| s.eq_ignore_ascii_case(stem));
        if !known || crate::receipt::validate_uuid(uuid_segment).is_err() {
            continue;
        }
        if remove_file(&entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression for the Phase 1 blocker: publishing a receipt failed with
    /// ACCESS_DENIED when the replace raced a real-time filter driver, and
    /// when a legacy delete-pending ghost still occupied the name. The exact
    /// delete/recreate/replace sequence of an uninstall/reinstall cycle must
    /// stay reliable at one path.
    #[test]
    fn repeated_delete_recreate_replace_never_denied() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("target")
            .join("paths-tests")
            .join(uuid::Uuid::new_v4().to_string());
        let dir = root.join("install");
        create_dir(&dir).unwrap();
        let path = dir.join("installation-receipt.json");
        for _ in 0..150 {
            atomic_write(&path, b"{\"gen\":1}").unwrap();
            atomic_write(&path, b"{\"gen\":2}").unwrap();
            assert_eq!(fs::read(&path).unwrap(), b"{\"gen\":2}");
            assert!(remove_file(&path).unwrap());
            assert!(!path.exists(), "delete left a delete-pending ghost name");
            atomic_write(&path, b"{\"gen\":3}").unwrap();
            atomic_write(&path, b"{\"gen\":4}").unwrap();
            assert_eq!(fs::read(&path).unwrap(), b"{\"gen\":4}");
            assert!(remove_file(&path).unwrap());
            assert!(!path.exists());
        }
        let residue: Vec<_> = fs::read_dir(&dir).unwrap().collect();
        assert!(residue.is_empty(), "stray temp files: {residue:?}");
        fs::remove_dir_all(&root).unwrap();
    }

    /// Deterministic sharing conflict (no AV timing, no sleeps beyond the
    /// production ladder itself): hold the destination open with no sharing
    /// at all, so every MoveFileExW attempt fails while the handle lives.
    #[test]
    fn move_replace_exhausts_bounded_retries_on_deterministic_conflict() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("target")
            .join("paths-tests")
            .join(uuid::Uuid::new_v4().to_string());
        let dir = root.join("install");
        create_dir(&dir).unwrap();
        let source = dir.join("source");
        let destination = dir.join("destination");
        fs::write(&source, b"new bytes").unwrap();
        fs::write(&destination, b"old bytes").unwrap();

        let conflict = OpenOptions::new().read(true).share_mode(0).open(&destination).unwrap();
        let error = move_replace(&source, &destination).unwrap_err();
        // Bounded ladder exhausted, never an unbounded wait.
        assert!(
            error.detail.contains("still denied after 6 retries"),
            "{error}"
        );
        // The error class stays precise: sharing violation or access denied —
        // exactly the retryable classes.
        assert!(
            error.detail.contains("0x80070020") || error.detail.contains("0x80070005"),
            "exact win32 class must be preserved: {error}"
        );
        // Contract: a failed replace leaves the caller's staged source intact
        // (the destination cannot be read while the exclusive conflict handle
        // is alive, so its content is checked right after the release).
        assert_eq!(fs::read(&source).unwrap(), b"new bytes");

        drop(conflict);
        assert_eq!(fs::read(&destination).unwrap(), b"old bytes");
        // An external filter driver may hold the freshly touched destination
        // a moment longer than the production ladder (the same documented
        // long-holder behavior); the production code must surface that, so
        // the bounded wait lives here in the test, not in the primitive.
        let mut replaced = move_replace(&source, &destination);
        for _ in 0..20 {
            if replaced.is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            replaced = move_replace(&source, &destination);
        }
        replaced.unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"new bytes");
        assert!(!source.exists());
        fs::remove_dir_all(&root).unwrap();
    }

    /// A non-retryable error must surface immediately as a precise per-file
    /// error, never enter the ACCESS_DENIED/sharing backoff ladder.
    #[test]
    fn move_replace_non_retryable_error_never_enters_the_backoff_ladder() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("target")
            .join("paths-tests")
            .join(uuid::Uuid::new_v4().to_string());
        let dir = root.join("install");
        create_dir(&dir).unwrap();
        let destination = dir.join("destination");
        fs::write(&destination, b"old bytes").unwrap();

        let error = move_replace(&dir.join("missing.source"), &destination).unwrap_err();
        assert!(error.detail.contains("Atomic replace"), "{error}");
        assert!(!error.detail.contains("still denied"), "{error}");
        assert_eq!(fs::read(&destination).unwrap(), b"old bytes");
        fs::remove_dir_all(&root).unwrap();
    }

    /// Crash-residue policy: only temps of the exact shape
    /// `<compiled-known stem>.<canonical uuid>.<tmp|installing>` are removed;
    /// unknown files, foreign stems, wrong uuid shapes, and wrong suffixes are
    /// preserved untouched.
    #[test]
    fn stale_transaction_temps_are_cleaned_and_unknown_files_preserved() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("target")
            .join("paths-tests")
            .join(uuid::Uuid::new_v4().to_string());
        let dir = root.join("install");
        create_dir(&dir).unwrap();
        let uuid = uuid::Uuid::new_v4().to_string();
        let residue = [
            format!("installation-receipt.{uuid}.tmp"),
            format!("desktop-todo-widget.{uuid}.installing"),
            format!("LICENSE.{uuid}.installing"),
            format!("active-uninstall.{uuid}.tmp"),
        ];
        let preserved = [
            format!("foo.{uuid}.tmp"),
            format!("installation-receipt.not-a-uuid.tmp"),
            format!("installation-receipt.{uuid}.tmp.bak"),
            format!("{uuid}.tmp"),
            "installation-receipt.json".to_string(),
            "notes.md".to_string(),
            "desktop-todo-widget.exe".to_string(),
        ];
        for name in residue.iter().chain(preserved.iter()) {
            fs::write(dir.join(name), b"x").unwrap();
        }

        let stems = [
            "installation-receipt",
            "desktop-todo-widget",
            "LICENSE",
            "active-uninstall",
        ];
        let removed = remove_stale_temps(&dir, &stems);
        assert_eq!(removed, 4, "exactly the four residue temps are removed");
        for name in &residue {
            assert!(!dir.join(name).exists(), "{name} must be removed");
        }
        for name in &preserved {
            assert!(dir.join(name).exists(), "{name} must be preserved");
        }
        // Idempotent: a second pass finds nothing left to remove, and the
        // preserved files stay untouched.
        assert_eq!(remove_stale_temps(&dir, &stems), 0);
        for name in &preserved {
            assert!(dir.join(name).exists(), "{name} must still be preserved");
        }
        fs::remove_dir_all(&root).unwrap();
    }
}
