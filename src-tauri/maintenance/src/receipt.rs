use crate::{
    paths::{self, Paths},
    resources::Resource,
    Error, ErrorKind, Result, APP_ID,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lifecycle {
    Installing,
    Installed,
    Updating,
    Uninstalling,
    RecoveryRequired,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileRecord {
    pub identity: Resource,
    pub version: String,
    pub size: u64,
    pub sha256: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Integration {
    MainStartMenuShortcut,
    WindowsUninstallEntry,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntegrationRecord {
    pub identity: Integration,
    pub present: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Metadata {
    pub updater_protocol: u32,
    pub receipt_generation: u64,
    pub active_session_id: Option<String>,
    pub last_completed_session_id: Option<String>,
    pub committed_manifest_sha256: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    pub schema_version: u32,
    pub app_id: String,
    pub installation_id: String,
    pub current_version: Option<String>,
    pub lifecycle_state: Lifecycle,
    pub install_root: PathBuf,
    pub data_root: PathBuf,
    pub installed_at: String,
    pub updated_at: String,
    pub runtime_resources: Vec<FileRecord>,
    pub support_resources: Vec<FileRecord>,
    pub integration_resources: Vec<IntegrationRecord>,
    pub maintenance: Metadata,
}
pub fn validate_uuid(value: &str) -> Result<()> {
    if uuid::Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value && !id.is_nil()) {
        Ok(())
    } else {
        Err(Error::new(
            ErrorKind::ReceiptMalformed,
            "Invalid canonical UUID",
        ))
    }
}
fn hash_valid(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn version_valid(s: &str) -> bool {
    let parts: Vec<_> = s.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|p| {
            !p.is_empty() && (p.len() == 1 || !p.starts_with('0')) && p.parse::<u16>().is_ok()
        })
}
pub fn fingerprint(path: &Path) -> Result<(u64, String)> {
    let mut f = paths::open_regular(path)?;
    let mut hash = Sha256::new();
    let mut size = 0;
    let mut bytes = [0u8; 65536];
    loop {
        let n = f.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
        size += n as u64;
    }
    Ok((size, format!("{:x}", hash.finalize())))
}
impl Receipt {
    pub fn new(
        paths: &Paths,
        version: &str,
        runtime_resources: Vec<FileRecord>,
        shortcut: bool,
    ) -> Self {
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        Self {
            schema_version: 1,
            app_id: APP_ID.into(),
            installation_id: uuid::Uuid::new_v4().to_string(),
            current_version: Some(version.into()),
            lifecycle_state: Lifecycle::Installing,
            install_root: paths.install().into(),
            data_root: paths.data().into(),
            installed_at: now.clone(),
            updated_at: now,
            runtime_resources,
            support_resources: vec![],
            integration_resources: vec![
                IntegrationRecord {
                    identity: Integration::MainStartMenuShortcut,
                    present: shortcut,
                },
                IntegrationRecord {
                    identity: Integration::WindowsUninstallEntry,
                    present: true,
                },
            ],
            maintenance: Metadata {
                updater_protocol: 1,
                receipt_generation: 1,
                active_session_id: None,
                last_completed_session_id: None,
                committed_manifest_sha256: None,
            },
        }
    }
    pub fn validate(&self, paths: &Paths) -> Result<()> {
        if self.schema_version != 1 || self.maintenance.updater_protocol != 1 {
            return Err(Error::new(
                ErrorKind::UnsupportedReceiptSchema,
                "Unsupported receipt schema/protocol",
            ));
        }
        paths.validate()?;
        if self.app_id != APP_ID {
            return Err(Error::new(
                ErrorKind::AppIdMismatch,
                "Receipt appId mismatch",
            ));
        }
        if !paths::equal(&self.install_root, paths.install()) {
            return Err(Error::new(
                ErrorKind::InstallRootMismatch,
                "Receipt install root mismatch",
            ));
        }
        if !paths::equal(&self.data_root, paths.data()) {
            return Err(Error::new(
                ErrorKind::DataRootMismatch,
                "Receipt data root mismatch",
            ));
        }
        validate_uuid(&self.installation_id)?;
        for id in [
            &self.maintenance.active_session_id,
            &self.maintenance.last_completed_session_id,
        ]
        .into_iter()
        .flatten()
        {
            validate_uuid(id)?;
        }
        if self.maintenance.receipt_generation == 0
            || self
                .maintenance
                .committed_manifest_sha256
                .as_deref()
                .is_some_and(|h| !hash_valid(h))
        {
            return Err(Error::new(
                ErrorKind::ReceiptMalformed,
                "Invalid maintenance metadata",
            ));
        }
        if !self.current_version.as_deref().is_some_and(version_valid)
            && !(self.current_version.is_none() && self.lifecycle_state == Lifecycle::Installing)
        {
            return Err(Error::new(
                ErrorKind::ReceiptMalformed,
                "Invalid current version",
            ));
        }
        for t in [&self.installed_at, &self.updated_at] {
            if !t.ends_with('Z') || chrono::DateTime::parse_from_rfc3339(t).is_err() {
                return Err(Error::new(
                    ErrorKind::ReceiptMalformed,
                    "Invalid UTC timestamp",
                ));
            }
        }
        let mut seen = HashSet::new();
        for (list, runtime) in [
            (&self.runtime_resources, true),
            (&self.support_resources, false),
        ] {
            for item in list {
                if item.identity.runtime() != runtime
                    || !seen.insert(item.identity)
                    || !version_valid(&item.version)
                    || !hash_valid(&item.sha256)
                    || item.size > 128 * 1024 * 1024
                {
                    return Err(Error::new(
                        ErrorKind::ReceiptMalformed,
                        "Invalid resource record",
                    ));
                }
            }
        }
        if self.runtime_resources.len() != 2 {
            return Err(Error::new(
                ErrorKind::ReceiptMalformed,
                "Expected two runtime identities",
            ));
        }
        let mut seen = HashSet::new();
        if self.integration_resources.len() != 2
            || self
                .integration_resources
                .iter()
                .any(|r| !seen.insert(r.identity))
        {
            return Err(Error::new(
                ErrorKind::ReceiptMalformed,
                "Invalid integration identities",
            ));
        }
        Ok(())
    }
    pub fn load(paths: &Paths) -> Result<Option<Self>> {
        let file = match paths::open_regular(&paths.receipt()) {
            Ok(f) => f,
            Err(e) if !paths.receipt().exists() && e.kind == ErrorKind::Io => return Ok(None),
            Err(e) => return Err(e),
        };
        let mut bytes = Vec::new();
        file.take(262145).read_to_end(&mut bytes)?;
        if bytes.len() > 262144 {
            return Err(Error::new(ErrorKind::ReceiptMalformed, "Receipt too large"));
        }
        let receipt: Self = serde_json::from_slice(&bytes)
            .map_err(|e| Error::new(ErrorKind::ReceiptMalformed, e.to_string()))?;
        receipt.validate(paths)?;
        Ok(Some(receipt))
    }
    pub fn save(&mut self, paths: &Paths) -> Result<()> {
        self.updated_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        self.maintenance.receipt_generation += 1;
        self.validate(paths)?;
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| Error::new(ErrorKind::ReceiptMalformed, e.to_string()))?;
        paths::atomic_write(&paths.receipt(), &bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Paths, Receipt) {
        let id = uuid::Uuid::new_v4().to_string();
        let paths = Paths::sandbox(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("target")
                .join("dtw-core-tests")
                .join(&id),
            &id,
        );
        paths::create_dir(paths.install()).unwrap();
        let files = [Resource::MainExecutable, Resource::MaintenanceHelper]
            .map(|identity| FileRecord {
                identity,
                version: "1.1.0".into(),
                size: 2,
                sha256: "a".repeat(64),
            })
            .to_vec();
        let receipt = Receipt::new(&paths, "1.1.0", files, true);
        (paths, receipt)
    }
    #[test]
    fn receipt_roundtrip_and_interrupted_temp() {
        let (p, mut r) = fixture();
        r.save(&p).unwrap();
        std::fs::write(
            p.install().join("installation-receipt.interrupted.tmp"),
            b"{",
        )
        .unwrap();
        let loaded = Receipt::load(&p).unwrap().unwrap();
        assert_eq!(loaded.installation_id, r.installation_id);
        r.lifecycle_state = Lifecycle::Installed;
        r.save(&p).unwrap();
        assert_eq!(
            Receipt::load(&p).unwrap().unwrap().lifecycle_state,
            Lifecycle::Installed
        );
    }
    #[test]
    fn rejects_bad_receipts_and_duplicate_fields() {
        let (p, r) = fixture();
        for mutate in [
            |r: &mut Receipt| r.schema_version = 2,
            |r: &mut Receipt| r.app_id = "other".into(),
            |r: &mut Receipt| r.install_root = "C:\\Windows".into(),
            |r: &mut Receipt| r.data_root = "C:\\".into(),
            |r: &mut Receipt| r.installation_id = "bad".into(),
            |r: &mut Receipt| r.runtime_resources[1].identity = Resource::MainExecutable,
        ] {
            let mut bad = r.clone();
            mutate(&mut bad);
            assert!(bad.validate(&p).is_err());
        }
        for bytes in ["{", "{\"schemaVersion\":1,\"schemaVersion\":1}"] {
            std::fs::write(p.receipt(), bytes).unwrap();
            assert!(Receipt::load(&p).is_err());
        }
        let text = serde_json::to_string(&r)
            .unwrap()
            .replace("Installing", "Unknown");
        std::fs::write(p.receipt(), text).unwrap();
        assert!(Receipt::load(&p).is_err());
    }
    #[test]
    fn exact_resources_and_assets() {
        assert_eq!(
            Resource::from_filename("DESKTOP-TODO-WIDGET.EXE"),
            Some(Resource::MainExecutable)
        );
        assert_eq!(
            Resource::from_filename("license_zh.md"),
            Some(Resource::LicenseZh)
        );
        for name in [
            "other.dll",
            "notes.md",
            "desktop-todo-widget.exe.bak",
            "../desktop-todo-widget.exe",
        ] {
            assert!(Resource::from_filename(name).is_none());
        }
        assert!(crate::resources::data_file("alan-desktop.sqlite3-wal"));
        assert!(!crate::resources::asset_file("background-unknown.png"));
        assert!(crate::resources::asset_file(&format!(
            "avatar-{}.png",
            uuid::Uuid::new_v4()
        )));
    }
    #[test]
    fn support_identities_never_count_as_runtime() {
        // Protocol 1 runtime integrity covers exactly the two executables;
        // distribution documents stay support/distribution resources even when
        // the receipt records their ownership.
        assert!(Resource::MainExecutable.runtime() && Resource::MaintenanceHelper.runtime());
        for support in [
            Resource::Readme,
            Resource::ReadmeZh,
            Resource::License,
            Resource::LicenseZh,
            Resource::ThirdPartyNotices,
        ] {
            assert!(!support.runtime(), "{support:?} must stay support-only");
            let (p, _) = fixture();
            let receipt = Receipt::new(
                &p,
                "1.1.0",
                vec![FileRecord {
                    identity: support,
                    version: "1.1.0".into(),
                    size: 2,
                    sha256: "a".repeat(64),
                }],
                false,
            );
            // A receipt claiming a support file as a runtime identity is
            // malformed: recorded lists must match the compiled policy.
            assert!(receipt.validate(&p).is_err());
        }
    }
    #[test]
    fn forged_identity_cannot_become_owned() {
        // Resource is a closed serde enum: a receipt text that names an
        // arbitrary path or invented identity fails deserialization, so a
        // forged receipt can never smuggle a new owned resource into the
        // ownership model.
        let (p, _) = fixture();
        let mut receipt = Receipt::new(&p, "1.1.0", vec![], false);
        receipt.support_resources = vec![FileRecord {
            identity: Resource::Readme,
            version: "1.1.0".into(),
            size: 2,
            sha256: "a".repeat(64),
        }];
        let text = serde_json::to_string(&receipt).unwrap();
        assert!(text.contains("\"readme\""));
        let forged = text.replace("\"readme\"", "\"..\\..\\arbitrary.dll\"");
        assert!(serde_json::from_str::<Receipt>(&forged).is_err());
        let invented = text.replace("\"readme\"", "\"notes\"");
        assert!(serde_json::from_str::<Receipt>(&invented).is_err());
    }
    #[test]
    fn unsafe_paths_and_hardlinks_are_rejected() {
        for path in [
            "C:\\Windows\\..\\file",
            "C:\\temp\\file:stream",
            "\\\\server\\share",
            "C:\\temp\\file.",
        ] {
            assert!(paths::pin_parents(Path::new(path)).is_err());
        }
        let (p, _) = fixture();
        let file = p.install().join("source");
        std::fs::write(&file, "test").unwrap();
        std::fs::hard_link(&file, p.install().join("alias")).unwrap();
        assert!(paths::open_regular(&file).is_err());
        assert!(paths::remove_file(&file).is_err());
    }
}
