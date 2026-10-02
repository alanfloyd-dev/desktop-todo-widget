//! Narrow, deterministic manifest generation (Phase 3B, pipeline step 3).
//!
//! Authoring policy (frozen decision for this round): **deterministic
//! compact-JSON generator** — field ordering is fixed, encoding is UTF-8
//! without BOM, there is no trailing newline, and the value is never
//! re-serialized. Once generated, the bytes are the bytes that are signed
//! and served; nothing downstream may pretty-print, reorder, or normalize
//! them.
//!
//! The generator emits exactly the frozen schema-1 field set — no new
//! fields, no optional extras — and then runs the produced bytes through
//! `update_core::validate_untrusted_manifest`. A generator that emits an
//! invalid manifest is a bug that fails closed here.

use crate::facts::ReleaseFacts;
use crate::PipelineError;

/// Bounded plain-text policy for `notes` (generator-side transport policy;
/// the compiled 256 KiB manifest bound remains the protocol-level limit).
const NOTES_MAX_CHARS: usize = 1000;

/// The full manifest generation input: validated release facts plus the
/// operator-supplied publication metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestInput {
    /// Target version, frozen grammar (three u16 components, no
    /// prerelease/build suffix, no leading zeros).
    pub version: String,
    /// RFC 3339 UTC timestamp (`...Z`).
    pub published_at: String,
    /// Bounded plain text; never executable HTML.
    pub notes: String,
}

/// Generate the manifest bytes from the final verified facts.
///
/// Byte shape (frozen field order, compact JSON, no trailing newline):
/// `schemaVersion, appId, channel, version, publishedAt, notes,
/// updaterProtocol, assets{windows-x64{filename,size,sha256,installFiles[
/// mainExecutable, maintenanceHelper]}}`.
pub fn generate_manifest_bytes(
    facts: &ReleaseFacts,
    input: &ManifestInput,
) -> Result<Vec<u8>, PipelineError> {
    // Generator-side grammar gates, using the one canonical comparator and
    // the one canonical timestamp rule — never a second parser.
    let version = desktop_todo_update_core::Version::parse(&input.version)
        .map_err(|e| PipelineError::ReleaseIdentity {
            detail: format!("version {:?} is not a valid protocol-1 target: {e}", input.version),
        })?;
    if !desktop_todo_update_core::rfc3339::is_rfc3339_utc(&input.published_at) {
        return Err(PipelineError::ReleaseIdentity {
            detail: format!(
                "publishedAt {:?} is not an RFC 3339 UTC timestamp",
                input.published_at
            ),
        });
    }
    if input.notes.chars().count() > NOTES_MAX_CHARS {
        return Err(PipelineError::ReleaseIdentity {
            detail: format!("notes exceed the {NOTES_MAX_CHARS}-character generator bound"),
        });
    }
    // The version that goes into the package filename must agree with the
    // target version (protocol: "version, package name and platform must
    // agree").
    let expected_filename = format!("desktop-todo-widget-v{version}-windows-x64.zip");
    if facts.package_filename != expected_filename {
        return Err(PipelineError::ReleaseIdentity {
            detail: format!(
                "package filename {:?} does not agree with the target version ({expected_filename:?})",
                facts.package_filename
            ),
        });
    }

    // The manifest's installFiles order is frozen (mainExecutable first,
    // maintenanceHelper second). The generator takes identity and filename
    // from the facts — never hard-codes them — and refuses a fact set that
    // is not exactly the two frozen managed identities in frozen order.
    if facts.install_files[0].identity != "mainExecutable"
        || facts.install_files[0].filename
            != desktop_todo_update_core::MAIN_EXECUTABLE_FILENAME
        || facts.install_files[1].identity != "maintenanceHelper"
        || facts.install_files[1].filename
            != desktop_todo_update_core::HELPER_EXECUTABLE_FILENAME
    {
        return Err(PipelineError::ReleaseIdentity {
            detail: "release facts do not carry exactly the two frozen managed identities                      (mainExecutable / maintenanceHelper) in frozen order"
                .to_string(),
        });
    }

    let notes_json = serde_json::to_string(&input.notes)
        .map_err(|e| PipelineError::ManifestInvalid { detail: e.to_string() })?;

    // Hand-serialized in the frozen field order: the manifest is a signed
    // document, so its byte shape is fixed by this generator and never
    // re-derived from a parsed value.
    let text = format!(
        concat!(
            r#"{{"schemaVersion":1,"appId":"{app_id}","channel":"stable","version":"{version}","#,
            r#""publishedAt":"{published_at}","notes":{notes},"updaterProtocol":1,"assets":{{"#,
            r#""windows-x64":{{"filename":"{filename}","size":{package_size},"sha256":"{package_sha}","installFiles":["#,
            r#"{{"identity":"{main_identity}","filename":"{main_name}","size":{main_size},"sha256":"{main_sha}"}},"#,
            r#"{{"identity":"{helper_identity}","filename":"{helper_name}","size":{helper_size},"sha256":"{helper_sha}"}}]}}}}}}"#
        ),
        app_id = facts.app_id,
        version = version,
        published_at = input.published_at,
        notes = notes_json,
        filename = facts.package_filename,
        package_size = facts.package_size,
        package_sha = facts.package_sha256_hex,
        main_identity = facts.install_files[0].identity,
        main_name = facts.install_files[0].filename,
        main_size = facts.install_files[0].size,
        main_sha = facts.install_files[0].sha256_hex,
        helper_identity = facts.install_files[1].identity,
        helper_name = facts.install_files[1].filename,
        helper_size = facts.install_files[1].size,
        helper_sha = facts.install_files[1].sha256_hex,
    );
    let bytes = text.into_bytes();

    // Fail closed: the generated bytes must pass the full frozen validation
    // exactly as the signer and every client will run it.
    desktop_todo_update_core::validate_untrusted_manifest(&bytes).map_err(|e| {
        PipelineError::ManifestInvalid {
            detail: e.to_string(),
        }
    })?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::compute_facts;
    use crate::assemble::assemble_package;
    use crate::{ReleaseMode, PackageInput};
    use desktop_todo_maintenance::package_zip::PACKAGE_ROOT_ALLOWLIST;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "release-pipeline-manifest-{}-{}-{label}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn build_facts(dir: &PathBuf) -> (PathBuf, ReleaseFacts) {
        let inputs_dir = dir.join("inputs");
        fs::create_dir_all(&inputs_dir).unwrap();
        let payloads = [
            ("desktop-todo-widget.exe", vec![1u8; 64]),
            ("desktop-todo-maintenance.exe", vec![2u8; 48]),
            ("install.ps1", b"install\n".to_vec()),
            ("uninstall.ps1", b"uninstall\n".to_vec()),
            ("README.md", b"r\n".to_vec()),
            ("README_ZH.md", b"rz\n".to_vec()),
            ("LICENSE", b"MIT\n".to_vec()),
            ("LICENSE_ZH.md", b"MITz\n".to_vec()),
            ("THIRD_PARTY_NOTICES.md", b"n\n".to_vec()),
        ];
        let inputs: Vec<PackageInput> = payloads
            .into_iter()
            .map(|(name, bytes)| {
                let path = inputs_dir.join(name);
                fs::write(&path, &bytes).unwrap();
                PackageInput {
                    entry_name: *PACKAGE_ROOT_ALLOWLIST
                        .iter()
                        .find(|allowed| **allowed == name)
                        .unwrap(),
                    source: path,
                }
            })
            .collect();
        let out = dir.join("desktop-todo-widget-v1.2.0-windows-x64.zip");
        assemble_package(&inputs, &out, false).unwrap();
        let facts = compute_facts(&out, ReleaseMode::Rehearsal, "1.2.0").unwrap();
        (out, facts)
    }

    fn manifest_input() -> ManifestInput {
        ManifestInput {
            version: "1.2.0".to_string(),
            published_at: "2026-10-02T00:00:00Z".to_string(),
            notes: "Application lifecycle management.".to_string(),
        }
    }

    #[test]
    fn same_facts_yield_byte_identical_manifests() {
        let dir = temp_dir("determinism");
        let (_, facts) = build_facts(&dir);
        let a = generate_manifest_bytes(&facts, &manifest_input()).unwrap();
        let b = generate_manifest_bytes(&facts, &manifest_input()).unwrap();
        assert_eq!(a, b);
        // And the frozen exact byte shape (golden): compact, fixed field
        // order, no whitespace, no trailing newline.
        let expected = format!(
            concat!(
                r#"{{"schemaVersion":1,"appId":"net.alanfloyd.desktop","channel":"stable","version":"1.2.0","#,
                r#""publishedAt":"2026-10-02T00:00:00Z","notes":"Application lifecycle management.","updaterProtocol":1,"assets":{{"#,
                r#""windows-x64":{{"filename":"desktop-todo-widget-v1.2.0-windows-x64.zip","size":{package_size},"sha256":"{package_sha}","installFiles":["#,
                r#"{{"identity":"mainExecutable","filename":"desktop-todo-widget.exe","size":{main_size},"sha256":"{main_sha}"}},"#,
                r#"{{"identity":"maintenanceHelper","filename":"desktop-todo-maintenance.exe","size":{helper_size},"sha256":"{helper_sha}"}}]}}}}}}"#
            ),
            package_size = facts.package_size,
            package_sha = facts.package_sha256_hex,
            main_size = facts.install_files[0].size,
            main_sha = facts.install_files[0].sha256_hex,
            helper_size = facts.install_files[1].size,
            helper_sha = facts.install_files[1].sha256_hex,
        );
        assert_eq!(String::from_utf8(a).unwrap(), expected);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn generated_manifest_passes_the_frozen_validator() {
        let dir = temp_dir("validator");
        let (_, facts) = build_facts(&dir);
        let bytes = generate_manifest_bytes(&facts, &manifest_input()).unwrap();
        desktop_todo_update_core::validate_untrusted_manifest(&bytes).unwrap();
        // Encoding rules: UTF-8, no BOM, no trailing newline.
        assert!(!bytes.starts_with(&[0xEF, 0xBB, 0xBF]));
        assert!(!bytes.ends_with(b"\n"));
        assert!(String::from_utf8(bytes).is_ok());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_version_grammar_is_rejected() {
        let dir = temp_dir("versions");
        let (_, facts) = build_facts(&dir);
        for bad in ["1.2", "01.2.0", "1.2.0-beta", "1.2.0+build", "a.b.c"] {
            let mut input = manifest_input();
            input.version = bad.to_string();
            let error = generate_manifest_bytes(&facts, &input).unwrap_err();
            assert!(matches!(error, PipelineError::ReleaseIdentity { .. }), "{bad}");
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn out_of_bound_or_malformed_facts_fail_the_frozen_validator() {
        let dir = temp_dir("tampered");
        let (_, mut facts) = build_facts(&dir);

        // A hash outside the frozen hex grammar is a typed validation
        // rejection.
        facts.install_files[0].sha256_hex = "not-hex".to_string();
        let error = generate_manifest_bytes(&facts, &manifest_input()).unwrap_err();
        assert!(matches!(error, PipelineError::ManifestInvalid { .. }));

        // An EXE size beyond the compiled limit is a typed rejection.
        let dir1 = temp_dir("tampered1");
        let (_, mut facts) = build_facts(&dir1);
        facts.install_files[0].size = 200_000_000;
        let error = generate_manifest_bytes(&facts, &manifest_input()).unwrap_err();
        assert!(matches!(error, PipelineError::ManifestInvalid { .. }));
        fs::remove_dir_all(&dir1).ok();

        // A within-bounds size tampering cannot be caught by the validator
        // (it does not know the package); the generated bytes simply differ
        // from the honest manifest, and read-back verification compares the
        // signed package facts against the actual package bytes.
        let dir2 = temp_dir("tampered2");
        let (_, facts_honest) = build_facts(&dir2);
        let mut facts_tampered = facts_honest.clone();
        facts_tampered.install_files[0].size += 1;
        let honest = generate_manifest_bytes(&facts_honest, &manifest_input()).unwrap();
        let tampered = generate_manifest_bytes(&facts_tampered, &manifest_input()).unwrap();
        assert_ne!(honest, tampered);
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&dir2).ok();
    }

    #[test]
    fn install_files_not_in_frozen_order_is_refused() {
        let dir = temp_dir("installfiles");
        let (_, mut facts) = build_facts(&dir);
        facts.install_files = [
            facts.install_files[1].clone(),
            facts.install_files[0].clone(),
        ];
        // The manifest shape is frozen with mainExecutable first; a fact set
        // in any other order (or with any other identity) is refused before
        // any bytes are emitted.
        let error = generate_manifest_bytes(&facts, &manifest_input()).unwrap_err();
        assert!(matches!(error, PipelineError::ReleaseIdentity { .. }));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn package_filename_must_agree_with_the_version() {
        let dir = temp_dir("agreement");
        let (_, mut facts) = build_facts(&dir);
        facts.package_filename = "desktop-todo-widget-v9.9.9-windows-x64.zip".to_string();
        let error = generate_manifest_bytes(&facts, &manifest_input()).unwrap_err();
        assert!(matches!(error, PipelineError::ReleaseIdentity { .. }));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn non_utc_timestamp_is_rejected() {
        let dir = temp_dir("timestamp");
        let (_, facts) = build_facts(&dir);
        let mut input = manifest_input();
        input.published_at = "2026-10-02T00:00:00+01:00".to_string();
        let error = generate_manifest_bytes(&facts, &input).unwrap_err();
        assert!(matches!(error, PipelineError::ReleaseIdentity { .. }));
        fs::remove_dir_all(&dir).ok();
    }
}
