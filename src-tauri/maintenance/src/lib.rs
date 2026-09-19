//! Local, offline lifecycle operations. No Tauri, network, or SQLite dependency.
pub mod integration;
pub mod lifecycle;
pub mod lock;
pub mod native;
pub mod paths;
pub mod receipt;
pub mod resources;
mod security;
pub const PRODUCT_VERSION: &str = env!("DTW_PRODUCT_VERSION");

use std::fmt;
pub const APP_ID: &str = "net.alanfloyd.desktop";
pub const MAIN_EXE: &str = "desktop-todo-widget.exe";
pub const HELPER_EXE: &str = "desktop-todo-maintenance.exe";
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidInstallation,
    UnsafePath,
    ReceiptMalformed,
    UnsupportedReceiptSchema,
    AppIdMismatch,
    InstallRootMismatch,
    DataRootMismatch,
    IntegrationFailure,
    LockUnavailable,
    MainProcessStillRunning,
    RuntimeRemovalFailure,
    PersistentDataRemovalFailure,
    PartialUninstall,
    Io,
}

#[derive(Debug)]
pub struct Error {
    pub kind: ErrorKind,
    pub detail: String,
}
impl Error {
    pub fn new(kind: ErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.detail)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::new(ErrorKind::Io, e.to_string())
    }
}
impl From<windows::core::Error> for Error {
    fn from(e: windows::core::Error) -> Self {
        Self::new(ErrorKind::Io, e.to_string())
    }
}
