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
                if attempt < WAITS_MS.len()
                    && matches!(
                        (e.code().0 as u32) & 0xffff,
                        x if x == ERROR_ACCESS_DENIED.0
                            || x == ERROR_SHARING_VIOLATION.0
                            || x == ERROR_LOCK_VIOLATION.0
                    ) =>
            {
                last = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(WAITS_MS[attempt]));
            }
            Err(e) => {
                return Err(Error::new(
                    ErrorKind::Io,
                    format!("Atomic replace {}: {e}", destination.display()),
                ))
            }
        }
    }
    Err(Error::new(
        ErrorKind::Io,
        format!(
            "Atomic replace {}: still denied after {} retries: {}",
            destination.display(),
            WAITS_MS.len(),
            last.expect("retry exhausted implies an error").to_string()
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
}
