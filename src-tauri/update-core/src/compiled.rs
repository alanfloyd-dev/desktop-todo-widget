//! Compiled protocol policy: identities, bounds, supported schema/protocol,
//! and the production trust store.
//!
//! Everything here is compiled into each binary. Runtime content — manifests,
//! envelopes, provider metadata, sessions, frontend state — can never widen
//! or change any of these values.

use crate::manifest::InstallIdentity;
use crate::trust::TrustStore;

/// The only application id protocol 1 accepts.
pub const APP_ID: &str = "net.alanfloyd.desktop";

/// The platform this client selects from the manifest assets map.
pub const CLIENT_PLATFORM: &str = "windows-x64";

/// The only manifest schemaVersion this parser understands.
pub const SUPPORTED_SCHEMA_VERSION: u32 = 1;

/// The only updaterProtocol this client can execute.
pub const SUPPORTED_UPDATER_PROTOCOL: u32 = 1;

/// The only delivery channel protocol 1 defines.
pub const SUPPORTED_CHANNEL: &str = "stable";

/// Compiled identity map: the two managed files, exactly as the release
/// package and the manifest `installFiles` entries must name them.
pub const MAIN_EXECUTABLE_FILENAME: &str = "desktop-todo-widget.exe";
pub const HELPER_EXECUTABLE_FILENAME: &str = "desktop-todo-maintenance.exe";

// Compiled resource limits (protocol v1, "Encoding and validation"). Every
// required size is also checked against these smaller compiled limits.
/// Maximum raw manifest document size.
pub const MANIFEST_MAX_BYTES: usize = 256 * 1024;
/// Maximum raw signature envelope size.
pub const ENVELOPE_MAX_BYTES: usize = 4 * 1024;
/// Maximum release package size.
pub const PACKAGE_MAX_BYTES: u64 = 256 * 1024 * 1024;
/// Maximum size of each managed executable.
pub const MANAGED_FILE_MAX_BYTES: u64 = 128 * 1024 * 1024;
/// Maximum total expanded data size.
pub const EXPANDED_MAX_BYTES: u64 = 512 * 1024 * 1024;

/// The filename a manifest `installFiles` identity must name.
pub fn identity_filename(identity: InstallIdentity) -> &'static str {
    match identity {
        InstallIdentity::MainExecutable => MAIN_EXECUTABLE_FILENAME,
        InstallIdentity::MaintenanceHelper => HELPER_EXECUTABLE_FILENAME,
    }
}

/// The production compiled trust store.
///
/// **Release-signing provisioning gate:** this store is intentionally empty
/// until the publisher provisions the real v1.2 Ed25519 public key(s) during
/// release preparation — no test key may ever be compiled here, and no
/// placeholder key is fabricated. With an empty store every candidate is
/// untrusted and every update fails closed, which is the correct behavior
/// for a client that cannot authenticate anyone yet. Populating this store
/// (as raw public keys, never private material) is a reviewed release gate
/// alongside the pinned repository addresses and signing credentials.
pub fn production_trust_store() -> TrustStore {
    TrustStore::empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;

    #[test]
    fn identity_map_covers_exactly_the_two_managed_files() {
        assert_eq!(
            identity_filename(InstallIdentity::MainExecutable),
            "desktop-todo-widget.exe"
        );
        assert_eq!(
            identity_filename(InstallIdentity::MaintenanceHelper),
            "desktop-todo-maintenance.exe"
        );
    }

    #[test]
    fn empty_production_store_is_fail_closed() {
        assert!(production_trust_store().is_empty());
        let manifest = format!(
            r#"{{"schemaVersion":1,"appId":"{APP_ID}","channel":"stable","version":"1.4.0","publishedAt":"2026-10-01T00:00:00Z","notes":"","updaterProtocol":1,"assets":{{}}}}"#
        );
        let result = crate::verify_and_parse(
            &production_trust_store(),
            format!(
                r#"{{"schemaVersion":1,"algorithm":"Ed25519","keyId":"{}","signature":"{}=="}}"#,
                "0".repeat(64),
                "A".repeat(86)
            )
            .as_bytes(),
            manifest.as_bytes(),
        );
        assert_eq!(result.unwrap_err().kind, ErrorKind::UntrustedKey);
    }
}
