use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Resource {
    MainExecutable,
    MaintenanceHelper,
    Readme,
    ReadmeZh,
    License,
    LicenseZh,
    ThirdPartyNotices,
}
impl Resource {
    pub fn filename(self) -> &'static str {
        match self {
            Self::MainExecutable => crate::MAIN_EXE,
            Self::MaintenanceHelper => crate::HELPER_EXE,
            Self::Readme => "README.md",
            Self::ReadmeZh => "README_ZH.md",
            Self::License => "LICENSE",
            Self::LicenseZh => "LICENSE_ZH.md",
            Self::ThirdPartyNotices => "THIRD_PARTY_NOTICES.md",
        }
    }
    pub fn runtime(self) -> bool {
        matches!(self, Self::MainExecutable | Self::MaintenanceHelper)
    }
    pub fn from_filename(name: &str) -> Option<Self> {
        [
            Self::MainExecutable,
            Self::MaintenanceHelper,
            Self::Readme,
            Self::ReadmeZh,
            Self::License,
            Self::LicenseZh,
            Self::ThirdPartyNotices,
        ]
        .into_iter()
        .find(|r| r.filename().eq_ignore_ascii_case(name))
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ownership {
    Runtime,
    Integration,
    PersistentData,
    Ephemeral,
    Unknown,
}
pub fn data_file(name: &str) -> bool {
    [
        "alan-desktop.sqlite3",
        "alan-desktop.sqlite3-wal",
        "alan-desktop.sqlite3-shm",
        "alan-desktop.sqlite3-journal",
    ]
    .iter()
    .any(|n| n.eq_ignore_ascii_case(name))
}
pub fn asset_file(name: &str) -> bool {
    let Some((base, ext)) = name.rsplit_once('.') else {
        return false;
    };
    let Some(id) = base
        .strip_prefix("avatar-")
        .or_else(|| base.strip_prefix("background-"))
    else {
        return false;
    };
    matches!(ext, "png" | "jpg" | "jpeg" | "webp") && crate::receipt::validate_uuid(id).is_ok()
}
