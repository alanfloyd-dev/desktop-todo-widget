//! The `ManifestV1` document: strict closed parsing and semantic validation.
//!
//! Parsing happens only after the signature over the exact raw bytes has
//! succeeded (the frozen verification order), so the parser can only ever
//! see the byte string the signature was verified over. The single open
//! dimension is the manifest `assets`/platform map: it may contain platform
//! keys this client does not recognize, and they are carried leniently —
//! but the selected `windows-x64` object itself is strictly closed.

use crate::compiled::{
    identity_filename, APP_ID, CLIENT_PLATFORM, EXPANDED_MAX_BYTES, MANAGED_FILE_MAX_BYTES,
    PACKAGE_MAX_BYTES, SUPPORTED_CHANNEL, SUPPORTED_SCHEMA_VERSION, SUPPORTED_UPDATER_PROTOCOL,
};
use crate::error::{ErrorKind, ProtocolError, SemanticViolation};
use crate::json::strict_parse;
use crate::rfc3339::is_rfc3339_utc;
use crate::sha256_hex;
use crate::trust::is_lowercase_hex_64;
use crate::version::Version;
use serde::Deserialize;
use std::collections::BTreeMap;

/// The exact raw bytes of `update-manifest.json` as served by the source.
/// The type exists so a verified byte string can be carried, persisted, and
/// re-verified without ever being reserialized.
#[derive(Debug, Clone)]
pub struct RawManifest {
    bytes: Vec<u8>,
}

impl RawManifest {
    /// Validate the encoding rules every protocol JSON document obeys
    /// (bounded size, UTF-8, no BOM) and take ownership of the bytes.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, ProtocolError> {
        if bytes.len() > crate::compiled::MANIFEST_MAX_BYTES {
            return Err(ProtocolError::new(
                ErrorKind::ManifestTooLarge,
                format!(
                    "manifest is {} bytes, limit is {}",
                    bytes.len(),
                    crate::compiled::MANIFEST_MAX_BYTES
                ),
            ));
        }
        if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
            return Err(ProtocolError::new(
                ErrorKind::ManifestBom,
                "manifest bytes carry a UTF-8 BOM".to_string(),
            ));
        }
        if std::str::from_utf8(&bytes).is_err() {
            return Err(ProtocolError::new(
                ErrorKind::ManifestNotUtf8,
                "manifest bytes are not valid UTF-8".to_string(),
            ));
        }
        Ok(Self { bytes })
    }

    /// The exact served bytes. Never a reserialization.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// The frozen manifest wire shape (schema 1). Top-level unknown fields are
/// rejected. The `assets` platform map is the one open dimension: platform
/// keys this client does not recognize are carried as uninterpreted values.
///
/// Deliberately crate-private: an unauthenticated `ManifestV1` value must
/// never be nameable by product code, or it could be mistaken for a
/// verified one. Parsed manifests escape this crate only as
/// `ValidatedManifest` inside an authenticated `VerifiedTarget`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestV1 {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    #[serde(rename = "appId")]
    app_id: String,
    channel: String,
    version: String,
    #[serde(rename = "publishedAt")]
    published_at: String,
    notes: String,
    #[serde(rename = "updaterProtocol")]
    updater_protocol: u32,
    assets: BTreeMap<String, serde_json::Value>,
}

/// The closed selected-platform asset object (strictly `windows-x64`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformAsset {
    filename: String,
    size: u64,
    sha256: String,
    #[serde(rename = "installFiles")]
    install_files: Vec<InstallFileEntry>,
}

impl PlatformAsset {
    pub fn filename(&self) -> &str {
        &self.filename
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn sha256_hex(&self) -> &str {
        &self.sha256
    }

    pub fn install_files(&self) -> &[InstallFileEntry] {
        &self.install_files
    }
}

/// One managed-file entry. The `identity` is parsed as text here so an
/// unknown identity produces the precise semantic violation instead of a
/// generic parse error.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallFileEntry {
    identity: String,
    filename: String,
    size: u64,
    sha256: String,
}

impl InstallFileEntry {
    pub fn identity(&self) -> &str {
        &self.identity
    }

    pub fn filename(&self) -> &str {
        &self.filename
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn sha256_hex(&self) -> &str {
        &self.sha256
    }
}

/// The closed installFiles identity enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallIdentity {
    MainExecutable,
    MaintenanceHelper,
}

impl InstallIdentity {
    pub fn from_manifest_text(text: &str) -> Option<Self> {
        match text {
            "mainExecutable" => Some(Self::MainExecutable),
            "maintenanceHelper" => Some(Self::MaintenanceHelper),
            _ => None,
        }
    }
}

/// A manifest that passed the full semantic validation. This is the only
/// parsed-manifest type this crate exposes, and it can only be constructed
/// inside the crate — the sole path is `verify_and_parse`, i.e. after
/// envelope validation, compiled-key lookup, and the exact raw-byte
/// signature check. There is no public or unchecked constructor by design:
/// external code can never hold a `ValidatedManifest` that did not come
/// from an authenticated verification.
#[derive(Debug)]
pub struct ValidatedManifest {
    manifest: ManifestV1,
    version: Version,
    asset: PlatformAsset,
}

impl ValidatedManifest {
    pub fn app_id(&self) -> &str {
        &self.manifest.app_id
    }

    pub fn channel(&self) -> &str {
        &self.manifest.channel
    }

    pub fn version(&self) -> &Version {
        &self.version
    }

    pub fn published_at(&self) -> &str {
        &self.manifest.published_at
    }

    pub fn notes(&self) -> &str {
        &self.manifest.notes
    }

    pub fn updater_protocol(&self) -> u32 {
        self.manifest.updater_protocol
    }

    /// The selected `windows-x64` asset.
    pub fn asset(&self) -> &PlatformAsset {
        &self.asset
    }
}

/// Strict-parse the manifest structure from the verified bytes. Closed
/// struct: unknown fields, duplicate keys, and trailing input are rejected.
///
/// Crate-private (audit F-1): exposing this publicly would let product code
/// obtain a parsed `ValidatedManifest` without ever passing envelope
/// validation, trust lookup, or the signature check.
pub(crate) fn parse_manifest(raw: &RawManifest) -> Result<ManifestV1, ProtocolError> {
    strict_parse(raw.bytes())
        .map_err(|e| ProtocolError::new(ErrorKind::ManifestMalformed, e.to_string()))
}

/// Semantic validation (step 6 of the frozen order). The failure kinds feed
/// the candidate policy: `UnsupportedUpdaterProtocol` and `PlatformMismatch`
/// make a candidate ineligible, every other kind makes it invalid.
///
/// Crate-private (audit F-1), for the same reason as `parse_manifest`: the
/// only public producer of `ValidatedManifest` is `verify_and_parse`.
pub(crate) fn validate_manifest(manifest: ManifestV1) -> Result<ValidatedManifest, ProtocolError> {
    let reject = |violation: SemanticViolation, detail: String| {
        Err(ProtocolError::new(
            ErrorKind::SemanticViolation(violation),
            detail,
        ))
    };

    if manifest.schema_version != SUPPORTED_SCHEMA_VERSION {
        return Err(ProtocolError::new(
            ErrorKind::ManifestSchemaUnsupported,
            format!(
                "manifest schemaVersion {} is not supported",
                manifest.schema_version
            ),
        ));
    }
    if manifest.app_id != APP_ID {
        return reject(
            SemanticViolation::AppIdMismatch,
            format!(
                "manifest appId {:?} is not this application",
                manifest.app_id
            ),
        );
    }
    if manifest.channel != SUPPORTED_CHANNEL {
        return reject(
            SemanticViolation::ChannelUnsupported,
            format!("manifest channel {:?} is not supported", manifest.channel),
        );
    }
    if manifest.updater_protocol != SUPPORTED_UPDATER_PROTOCOL {
        return Err(ProtocolError::new(
            ErrorKind::UnsupportedUpdaterProtocol,
            format!(
                "manifest updaterProtocol {} is not executable by this client",
                manifest.updater_protocol
            ),
        ));
    }
    let version = Version::parse(&manifest.version)?;
    if !is_rfc3339_utc(&manifest.published_at) {
        return reject(
            SemanticViolation::PublishedAtMalformed,
            format!(
                "publishedAt {:?} is not an RFC 3339 UTC timestamp",
                manifest.published_at
            ),
        );
    }

    // The platform map may carry unknown platform keys; this client selects
    // exactly its own. A missing windows-x64 asset is a platform mismatch,
    // not document corruption.
    let asset_value = match manifest.assets.get(CLIENT_PLATFORM) {
        Some(value) => value,
        None => {
            return Err(ProtocolError::new(
                ErrorKind::PlatformMismatch,
                format!("manifest carries no {CLIENT_PLATFORM} asset"),
            ));
        }
    };
    let asset: PlatformAsset = serde_json::from_value(asset_value.clone()).map_err(|e| {
        ProtocolError::new(
            ErrorKind::SemanticViolation(SemanticViolation::SelectedAssetMalformed),
            format!("{CLIENT_PLATFORM} asset object violates the closed schema: {e}"),
        )
    })?;

    validate_platform_asset(&asset)?;

    Ok(ValidatedManifest {
        manifest,
        version,
        asset,
    })
}

fn validate_platform_asset(asset: &PlatformAsset) -> Result<(), ProtocolError> {
    let reject = |violation: SemanticViolation, detail: String| {
        Err(ProtocolError::new(
            ErrorKind::SemanticViolation(violation),
            detail,
        ))
    };

    // The package filename is a required closed-schema field carried into
    // the frozen tuple: an opaque identity/locator, not a grammar-checked
    // name. Protocol 1 freezes only that "version, package name and
    // platform must agree" and no concrete filename grammar, so no naming
    // shape is a semantic rejection here. The later package phase checks
    // the downloaded object against the already-signed metadata (filename
    // equality, size, SHA-256) and the compiled package-content allowlist;
    // the package bytes themselves are never "signed", and schema 1 never
    // re-introduces an unfrozen naming format.
    if asset.size > PACKAGE_MAX_BYTES {
        return reject(
            SemanticViolation::PackageSizeOutOfRange,
            format!("package size {} exceeds the compiled limit", asset.size),
        );
    }
    if !is_lowercase_hex_64(&asset.sha256) {
        return reject(
            SemanticViolation::PackageSha256Malformed,
            "package sha256 must be 64 lowercase hex characters".to_string(),
        );
    }

    // Exactly the two fixed identities, each exactly once.
    if asset.install_files.len() != 2 {
        return reject(
            SemanticViolation::InstallFileMissing,
            format!(
                "installFiles carries {} entries, exactly the two fixed identities are required",
                asset.install_files.len()
            ),
        );
    }
    let mut seen_identities: Vec<InstallIdentity> = Vec::new();
    let mut expanded_total: u64 = 0;
    for entry in &asset.install_files {
        let identity = match InstallIdentity::from_manifest_text(&entry.identity) {
            Some(identity) => identity,
            None => {
                return reject(
                    SemanticViolation::InstallFileIdentityUnknown,
                    format!(
                        "installFiles identity {:?} is not a managed resource",
                        entry.identity
                    ),
                )
            }
        };
        if seen_identities.contains(&identity) {
            return reject(
                SemanticViolation::InstallFileDuplicate,
                format!(
                    "installFiles identity {:?} appears more than once",
                    entry.identity
                ),
            );
        }
        seen_identities.push(identity);
        if entry.filename != identity_filename(identity) {
            return reject(
                SemanticViolation::InstallFileFilenameMismatch,
                format!(
                    "installFiles identity {:?} must name {:?}, found {:?}",
                    entry.identity,
                    identity_filename(identity),
                    entry.filename
                ),
            );
        }
        if entry.size > MANAGED_FILE_MAX_BYTES {
            return reject(
                SemanticViolation::InstallFileSizeOutOfRange,
                format!(
                    "installFiles entry {:?} size {} exceeds the compiled limit",
                    entry.identity, entry.size
                ),
            );
        }
        if !is_lowercase_hex_64(&entry.sha256) {
            return reject(
                SemanticViolation::InstallFileSha256Malformed,
                format!(
                    "installFiles entry {:?} sha256 must be 64 lowercase hex characters",
                    entry.identity
                ),
            );
        }
        expanded_total = expanded_total.saturating_add(entry.size);
    }
    if expanded_total > EXPANDED_MAX_BYTES {
        return reject(
            SemanticViolation::ExpandedSizeOutOfRange,
            format!("total expanded size {expanded_total} exceeds the compiled limit"),
        );
    }
    Ok(())
}

/// Convenience: the manifest SHA-256 digest as lowercase hex, computed over
/// the exact raw bytes at freeze time.
pub fn manifest_digest_hex(raw: &RawManifest) -> String {
    sha256_hex(raw.bytes())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn manifest_text(version: &str) -> String {
        format!(
            concat!(
                r#"{{"schemaVersion":1,"appId":"net.alanfloyd.desktop","channel":"stable","version":"{v}","#,
                r#""publishedAt":"2026-10-01T00:00:00Z","notes":"Application lifecycle management.","updaterProtocol":1,"assets":{{"#,
                r#""windows-x64":{{"filename":"desktop-todo-widget-v{v}-windows-x64.zip","size":3000000,"sha256":"{h}","installFiles":["#,
                r#"{{"identity":"mainExecutable","filename":"desktop-todo-widget.exe","size":5500000,"sha256":"{h}"}},"#,
                r#"{{"identity":"maintenanceHelper","filename":"desktop-todo-maintenance.exe","size":800000,"sha256":"{h}"}}]}},"#,
                r#""linux-x64":{{"filename":"other","size":1,"sha256":"{h}","installFiles":[]}}}}}}"#
            ),
            v = version,
            h = "a".repeat(64)
        )
    }

    pub(crate) fn valid_manifest() -> RawManifest {
        RawManifest::from_bytes(manifest_text("1.4.0").into_bytes()).unwrap()
    }

    #[test]
    fn valid_manifest_parses_and_validates() {
        let raw = valid_manifest();
        let validated = validate_manifest(parse_manifest(&raw).unwrap()).unwrap();
        assert_eq!(validated.version().to_string(), "1.4.0");
        assert_eq!(
            validated.asset().filename(),
            "desktop-todo-widget-v1.4.0-windows-x64.zip"
        );
        assert_eq!(validated.asset().install_files().len(), 2);
        assert_eq!(validated.app_id(), APP_ID);
        assert_eq!(validated.updater_protocol(), SUPPORTED_UPDATER_PROTOCOL);
    }

    #[test]
    fn unknown_top_level_fields_are_rejected() {
        let text = manifest_text("1.4.0").replace(
            r#""notes":"Application lifecycle management.","#,
            r#""notes":"","extra":1,"#,
        );
        let raw = RawManifest::from_bytes(text.into_bytes()).unwrap();
        assert_eq!(
            parse_manifest(&raw).unwrap_err().kind,
            ErrorKind::ManifestMalformed
        );
    }

    #[test]
    fn selected_asset_object_is_closed() {
        // The unknown linux-x64 platform carries an unknown field and is
        // ignored; the same unknown field on windows-x64 is rejected.
        let text = manifest_text("1.4.0").replace(
            r#""windows-x64":{"filename""#,
            r#""windows-x64":{"extra":1,"filename""#,
        );
        let raw = RawManifest::from_bytes(text.into_bytes()).unwrap();
        let parsed = parse_manifest(&raw).unwrap();
        assert_eq!(
            validate_manifest(parsed).unwrap_err().kind,
            ErrorKind::SemanticViolation(SemanticViolation::SelectedAssetMalformed)
        );
    }

    #[test]
    fn semantic_violations_are_typed() {
        // Unknown installFiles identity.
        let text = manifest_text("1.4.0").replace("maintenanceHelper", "helperExe");
        let parsed = parse_manifest(&RawManifest::from_bytes(text.into_bytes()).unwrap()).unwrap();
        assert_eq!(
            validate_manifest(parsed).unwrap_err().kind,
            ErrorKind::SemanticViolation(SemanticViolation::InstallFileIdentityUnknown)
        );

        // The package filename is opaque at the manifest layer: protocol 1
        // froze the agreement requirement, never a concrete grammar, so a
        // differently named package file is NOT a semantic rejection.
        let text =
            manifest_text("1.4.0").replace("desktop-todo-widget-v1.4.0-windows-x64.zip", "pkg.zip");
        let parsed = parse_manifest(&RawManifest::from_bytes(text.into_bytes()).unwrap()).unwrap();
        let validated = validate_manifest(parsed).unwrap();
        assert_eq!(validated.asset().filename(), "pkg.zip");

        // Wrong version grammar is a version error, not a semantic one.
        let text = manifest_text("1.4.0-beta");
        let parsed = parse_manifest(&RawManifest::from_bytes(text.into_bytes()).unwrap()).unwrap();
        assert_eq!(
            validate_manifest(parsed).unwrap_err().kind,
            ErrorKind::VersionMalformed
        );

        // Unsupported updaterProtocol (candidate-ineligible).
        let text =
            manifest_text("1.4.0").replace(r#""updaterProtocol":1"#, r#""updaterProtocol":2"#);
        let parsed = parse_manifest(&RawManifest::from_bytes(text.into_bytes()).unwrap()).unwrap();
        assert_eq!(
            validate_manifest(parsed).unwrap_err().kind,
            ErrorKind::UnsupportedUpdaterProtocol
        );

        // Missing windows-x64 asset (candidate-ineligible): rename the
        // platform key so this client finds no applicable platform.
        let text = manifest_text("1.4.0").replace(
            r#""windows-x64":{"filename":"desktop-todo-widget-v1.4.0-windows-x64.zip""#,
            r#""freebsd-x64":{"filename":"desktop-todo-widget-v1.4.0-windows-x64.zip""#,
        );
        let parsed = parse_manifest(&RawManifest::from_bytes(text.into_bytes()).unwrap()).unwrap();
        assert_eq!(
            validate_manifest(parsed).unwrap_err().kind,
            ErrorKind::PlatformMismatch
        );

        // Unsupported schemaVersion (candidate-invalid).
        let text = manifest_text("1.4.0").replace(r#""schemaVersion":1"#, r#""schemaVersion":2"#);
        let parsed = parse_manifest(&RawManifest::from_bytes(text.into_bytes()).unwrap()).unwrap();
        assert_eq!(
            validate_manifest(parsed).unwrap_err().kind,
            ErrorKind::ManifestSchemaUnsupported
        );
    }

    /// Regression pins (audit F-2A): every remaining semantic rejection is
    /// wired to its exact typed variant, never a bare `is_err()`.
    #[test]
    fn remaining_semantic_violations_are_typed() {
        let reject = |text: String| {
            let raw = RawManifest::from_bytes(text.into_bytes()).unwrap();
            validate_manifest(parse_manifest(&raw).unwrap())
                .unwrap_err()
                .kind
        };

        // AppIdMismatch.
        assert_eq!(
            reject(manifest_text("1.4.0").replace("net.alanfloyd.desktop", "net.evil.desktop")),
            ErrorKind::SemanticViolation(SemanticViolation::AppIdMismatch)
        );
        // ChannelUnsupported.
        assert_eq!(
            reject(manifest_text("1.4.0").replace(r#""channel":"stable""#, r#""channel":"beta""#)),
            ErrorKind::SemanticViolation(SemanticViolation::ChannelUnsupported)
        );
        // PublishedAtMalformed: a non-UTC offset is not an RFC 3339 UTC
        // timestamp.
        assert_eq!(
            reject(
                manifest_text("1.4.0").replace("2026-10-01T00:00:00Z", "2026-10-01T00:00:00+01:00")
            ),
            ErrorKind::SemanticViolation(SemanticViolation::PublishedAtMalformed)
        );
        // PackageSizeOutOfRange: beyond the frozen 256 MiB package limit.
        assert_eq!(
            reject(manifest_text("1.4.0").replace(r#""size":3000000"#, r#""size":300000000"#)),
            ErrorKind::SemanticViolation(SemanticViolation::PackageSizeOutOfRange)
        );
        // InstallFileSizeOutOfRange: beyond the frozen 128 MiB per-EXE limit.
        assert_eq!(
            reject(manifest_text("1.4.0").replace(r#""size":5500000"#, r#""size":200000000"#)),
            ErrorKind::SemanticViolation(SemanticViolation::InstallFileSizeOutOfRange)
        );
    }

    /// Regression pin (audit F-2A): the 512 MiB expanded-size rejection is
    /// entailed by the smaller frozen limits under schema 1's fixed
    /// two-entry shape, so it can never fire through a legal manifest — the
    /// check stays as a compiled backstop, and any attempt to exceed the
    /// budget trips an earlier typed rejection first.
    #[test]
    fn expanded_size_limit_is_entailed_and_unreachable_under_schema_1() {
        // Two managed files at exactly the per-file compiled limit (128 MiB
        // each) pass: 256 MiB total is within the 512 MiB expanded budget.
        let text = manifest_text("1.4.0")
            .replace(r#""size":5500000"#, r#""size":134217728"#)
            .replace(r#""size":800000"#, r#""size":134217728"#);
        let raw = RawManifest::from_bytes(text.into_bytes()).unwrap();
        assert!(validate_manifest(parse_manifest(&raw).unwrap()).is_ok());

        // Any total beyond the budget requires a per-file size beyond the
        // 128 MiB limit, which rejects first with its own typed variant.
        let text = manifest_text("1.4.0")
            .replace(r#""size":5500000"#, r#""size":200000000"#)
            .replace(r#""size":800000"#, r#""size":200000000"#);
        let raw = RawManifest::from_bytes(text.into_bytes()).unwrap();
        assert_eq!(
            validate_manifest(parse_manifest(&raw).unwrap())
                .unwrap_err()
                .kind,
            ErrorKind::SemanticViolation(SemanticViolation::InstallFileSizeOutOfRange)
        );
    }

    #[test]
    fn raw_manifest_rejects_bom_and_invalid_utf8() {
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice(manifest_text("1.4.0").as_bytes());
        assert_eq!(
            RawManifest::from_bytes(bom).unwrap_err().kind,
            ErrorKind::ManifestBom
        );
        let mut invalid = manifest_text("1.4.0").into_bytes();
        invalid[0] = 0xFF;
        assert_eq!(
            RawManifest::from_bytes(invalid).unwrap_err().kind,
            ErrorKind::ManifestNotUtf8
        );
        let oversized = vec![b'a'; crate::compiled::MANIFEST_MAX_BYTES + 1];
        assert_eq!(
            RawManifest::from_bytes(oversized).unwrap_err().kind,
            ErrorKind::ManifestTooLarge
        );
    }

    #[test]
    fn manifest_digest_is_over_exact_bytes() {
        let raw = valid_manifest();
        assert_eq!(manifest_digest_hex(&raw).len(), 64);
        assert_eq!(manifest_digest_hex(&raw), sha256_hex(raw.bytes()));
    }
}
