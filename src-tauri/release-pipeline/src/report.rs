//! Local release reports (Phase 3B, §20).
//!
//! Three JSON documents live in the staging root. None of them is a
//! protocol artifact, none of them is uploaded, and none of them carries
//! local paths or secrets beyond the operator's own machine:
//!
//! - `facts.json` — the machine-readable release facts (step outputs;
//!   artifact filenames, sizes, digests, mode);
//! - `report.json` — the prepare-phase release report (build contract,
//!   commit, artifact names, mode, provider targets, read-back results);
//! - `verification-report.json` — the read-back verification outcome per
//!   provider, written by the read-back step and consumed by `finalize`.

use std::path::{Path, PathBuf};

use crate::facts::ReleaseFacts;
use crate::publish::ReadBackResult;
use crate::{ReleaseMode, PipelineError};

/// Serialize the release facts to `facts.json`.
pub fn write_facts_json(staging: &Path, facts: &ReleaseFacts) -> Result<PathBuf, PipelineError> {
    let value = serde_json::json!({
        "note": "operator/tooling state; not a protocol artifact; never uploaded",
        "mode": facts.mode.as_str(),
        "appId": facts.app_id,
        "updaterProtocol": facts.updater_protocol,
        "platform": facts.platform,
        "targetVersion": facts.target_version,
        "package": {
            "filename": facts.package_filename,
            "size": facts.package_size,
            "sha256": facts.package_sha256_hex,
        },
        "installFiles": facts.install_files.iter().map(|entry| serde_json::json!({
            "identity": entry.identity,
            "filename": entry.filename,
            "size": entry.size,
            "sha256": entry.sha256_hex,
        })).collect::<Vec<_>>(),
        "manifestSha256": facts.manifest_sha256,
        "envelopeKeyId": facts.envelope_key_id,
    });
    let bytes = serde_json::to_vec_pretty(&value).map_err(|e| PipelineError::Io {
        detail: e.to_string(),
    })?;
    let path = staging.join("facts.json");
    std::fs::write(&path, bytes).map_err(|e| PipelineError::Io {
        detail: e.to_string(),
    })?;
    Ok(path)
}

/// Read the recorded release mode from an existing `facts.json`. Absent
/// file → `None` (fresh staging root).
pub fn read_staging_mode(staging: &Path) -> Result<Option<ReleaseMode>, PipelineError> {
    let path = staging.join("facts.json");
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path).map_err(|e| PipelineError::Io {
        detail: e.to_string(),
    })?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
        PipelineError::StagingInvalid {
            path: path.clone(),
            detail: format!("facts.json is not valid JSON: {e}"),
        }
    })?;
    match value.get("mode").and_then(|mode| mode.as_str()) {
        Some("production") => Ok(Some(ReleaseMode::Production)),
        Some("rehearsal") => Ok(Some(ReleaseMode::Rehearsal)),
        other => Err(PipelineError::StagingInvalid {
            path,
            detail: format!("facts.json carries an unknown release mode {other:?}"),
        }),
    }
}

/// The prepare-phase release report (§20). Provider targets and read-back
/// results are updated by later steps and rewritten in place.
#[derive(Debug, Clone)]
pub struct ReleaseReport {
    pub mode: ReleaseMode,
    pub target_version: String,
    pub source_commit: Option<String>,
    pub artifact_filenames: Vec<String>,
    pub provider_targets: Vec<String>,
    /// Local staging root (a local report field only — never uploaded).
    pub staging_root: String,
}

/// Serialize the release report to `report.json`.
pub fn write_release_report(
    staging: &Path,
    report: &ReleaseReport,
) -> Result<PathBuf, PipelineError> {
    let value = serde_json::json!({
        "note": "local operator report; not a protocol artifact; never uploaded",
        "mode": report.mode.as_str(),
        "targetVersion": report.target_version,
        "sourceCommit": report.source_commit,
        "buildCommands": crate::facts::CANONICAL_BUILD_COMMANDS,
        "artifactFilenames": report.artifact_filenames,
        "providerTargets": report.provider_targets,
        "stagingRoot": report.staging_root,
        "readBack": serde_json::Value::Null,
    });
    let bytes = serde_json::to_vec_pretty(&value).map_err(|e| PipelineError::Io {
        detail: e.to_string(),
    })?;
    let path = staging.join("report.json");
    std::fs::write(&path, bytes).map_err(|e| PipelineError::Io {
        detail: e.to_string(),
    })?;
    Ok(path)
}

/// One read-back verification record for `verification-report.json`.
#[derive(Debug, Clone)]
pub struct VerificationRecord {
    pub provider: &'static str,
    pub tag: String,
    pub mode: ReleaseMode,
    pub verified: bool,
    pub manifest_sha256_hex: String,
    pub key_id: String,
    pub artifact_sha256: Vec<(String, String)>,
    pub detail: String,
}

impl VerificationRecord {
    pub fn from_result(
        result: &ReadBackResult,
        tag: &str,
        mode: ReleaseMode,
    ) -> VerificationRecord {
        VerificationRecord {
            provider: result.provider,
            tag: tag.to_string(),
            mode,
            verified: true,
            manifest_sha256_hex: result.manifest_sha256_hex.clone(),
            key_id: result.key_id.clone(),
            artifact_sha256: result.artifact_sha256.clone(),
            detail: String::new(),
        }
    }

    pub fn failure(provider: &'static str, tag: &str, mode: ReleaseMode, detail: String) -> Self {
        VerificationRecord {
            provider,
            tag: tag.to_string(),
            mode,
            verified: false,
            manifest_sha256_hex: String::new(),
            key_id: String::new(),
            artifact_sha256: Vec::new(),
            detail,
        }
    }
}

/// Serialize the verification report, recording every provider's outcome.
/// The gate rule this report encodes: only after **both** providers
/// verified is the release eligible for exposure to updater discovery.
pub fn write_verification_report(
    staging: &Path,
    records: &[VerificationRecord],
) -> Result<PathBuf, PipelineError> {
    let all_verified = !records.is_empty() && records.iter().all(|record| record.verified);
    let value = serde_json::json!({
        "note": "local read-back verification report; not a protocol artifact; never uploaded",
        "allProvidersVerified": all_verified,
        "records": records.iter().map(|record| serde_json::json!({
            "provider": record.provider,
            "tag": record.tag,
            "mode": record.mode.as_str(),
            "verified": record.verified,
            "manifestSha256": record.manifest_sha256_hex,
            "keyId": record.key_id,
            "artifactSha256": record.artifact_sha256.iter().map(|(name, sha)| serde_json::json!({
                "name": name,
                "sha256": sha,
            })).collect::<Vec<_>>(),
            "detail": record.detail,
        })).collect::<Vec<_>>(),
    });
    let bytes = serde_json::to_vec_pretty(&value).map_err(|e| PipelineError::Io {
        detail: e.to_string(),
    })?;
    let path = staging.join("verification-report.json");
    std::fs::write(&path, bytes).map_err(|e| PipelineError::Io {
        detail: e.to_string(),
    })?;
    Ok(path)
}

/// Read the verification report and return whether the given provider
/// verified for the given tag (used by `finalize` to refuse exposing a
/// release that has not passed read-back).
pub fn provider_verified(staging: &Path, provider: &str, tag: &str) -> Result<bool, PipelineError> {
    let path = staging.join("verification-report.json");
    if !path.exists() {
        return Ok(false);
    }
    let bytes = std::fs::read(&path).map_err(|e| PipelineError::Io {
        detail: e.to_string(),
    })?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
        PipelineError::StagingInvalid {
            path: path.clone(),
            detail: format!("verification-report.json is not valid JSON: {e}"),
        }
    })?;
    Ok(value
        .get("records")
        .and_then(|records| records.as_array())
        .map(|records| {
            records.iter().any(|record| {
                record.get("provider").and_then(|p| p.as_str()) == Some(provider)
                    && record.get("tag").and_then(|t| t.as_str()) == Some(tag)
                    && record.get("verified").and_then(|v| v.as_bool()) == Some(true)
            })
        })
        .unwrap_or(false))
}

/// The sha256sum-style sidecar content for the manual bootstrap digest
/// check: `<64 lowercase hex>  <filename>` with a trailing newline.
pub fn sidecar_content(package_filename: &str, sha256_hex: &str) -> String {
    format!("{sha256_hex}  {package_filename}\n")
}

/// Load the release facts previously written by `prepare` (used by the
/// publish/read-back commands and by tests). The staging `facts.json` is
/// operator state, so this is a plain reader — it grants no authority.
pub fn read_facts(staging: &Path) -> Result<ReleaseFacts, PipelineError> {
    let path = staging.join("facts.json");
    let invalid = |detail: String| PipelineError::StagingInvalid {
        path: path.clone(),
        detail,
    };
    let bytes = std::fs::read(&path).map_err(|e| {
        invalid(format!("facts.json is missing or unreadable — run `prepare` first ({e})"))
    })?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| invalid(format!("facts.json is not valid JSON: {e}")))?;

    let text_field = |container: &serde_json::Value, field: &str| -> Result<String, PipelineError> {
        container
            .get(field)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| invalid(format!("facts.json field {field:?} is missing or not a string")))
    };
    let number_field = |container: &serde_json::Value, field: &str| -> Result<u64, PipelineError> {
        container
            .get(field)
            .and_then(|v| v.as_u64())
            .ok_or_else(|| invalid(format!("facts.json field {field:?} is missing or not a number")))
    };

    let mode = match text_field(&value, "mode")?.as_str() {
        "production" => ReleaseMode::Production,
        "rehearsal" => ReleaseMode::Rehearsal,
        other => {
            return Err(invalid(format!(
                "facts.json carries an unknown release mode {other:?}"
            )))
        }
    };
    let package = value
        .get("package")
        .cloned()
        .ok_or_else(|| invalid("facts.json is missing package".to_string()))?;
    let install_files = value
        .get("installFiles")
        .and_then(|files| files.as_array())
        .ok_or_else(|| invalid("facts.json is missing installFiles".to_string()))?;
    if install_files.len() != 2 {
        return Err(invalid(
            "facts.json must carry exactly the two managed identities".to_string(),
        ));
    }
    let file_entry = |file: &serde_json::Value| -> Result<crate::facts::InstallFileFact, PipelineError> {
        Ok(crate::facts::InstallFileFact {
            identity: text_field(file, "identity")?,
            filename: text_field(file, "filename")?,
            size: number_field(file, "size")?,
            sha256_hex: text_field(file, "sha256")?,
        })
    };
    Ok(ReleaseFacts {
        mode,
        package_filename: text_field(&package, "filename")?,
        package_size: number_field(&package, "size")?,
        package_sha256_hex: text_field(&package, "sha256")?,
        install_files: [file_entry(&install_files[0])?, file_entry(&install_files[1])?],
        target_version: text_field(&value, "targetVersion")?,
        app_id: text_field(&value, "appId")?,
        updater_protocol: number_field(&value, "updaterProtocol")? as u32,
        platform: text_field(&value, "platform")?,
        manifest_sha256: value
            .get("manifestSha256")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        envelope_key_id: value
            .get("envelopeKeyId")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        build_commands: crate::facts::CANONICAL_BUILD_COMMANDS,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::compute_facts;
    use crate::assemble::assemble_package;
    use crate::PackageInput;
    use desktop_todo_maintenance::package_zip::PACKAGE_ROOT_ALLOWLIST;
    use std::fs;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "release-pipeline-report-{}-{}-{label}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn build_facts(staging: &Path) -> ReleaseFacts {
        let inputs_dir = staging.join("inputs");
        fs::create_dir_all(&inputs_dir).unwrap();
        let payloads = [
            ("desktop-todo-widget.exe", vec![1u8; 64]),
            ("desktop-todo-maintenance.exe", vec![2u8; 48]),
            ("install.ps1", b"i\n".to_vec()),
            ("uninstall.ps1", b"u\n".to_vec()),
            ("README.md", b"r\n".to_vec()),
            ("README_ZH.md", b"rz\n".to_vec()),
            ("LICENSE", b"M\n".to_vec()),
            ("LICENSE_ZH.md", b"Mz\n".to_vec()),
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
        let out = staging.join("desktop-todo-widget-v1.2.0-windows-x64.zip");
        assemble_package(&inputs, &out, false).unwrap();
        compute_facts(&out, ReleaseMode::Rehearsal, "1.2.0").unwrap()
    }

    #[test]
    fn facts_json_round_trips_the_mode() {
        let dir = temp_dir("mode");
        let facts = build_facts(&dir);
        write_facts_json(&dir, &facts).unwrap();
        assert_eq!(
            read_staging_mode(&dir).unwrap(),
            Some(ReleaseMode::Rehearsal)
        );
        // A different mode into the same staging root is a conflict.
        let conflict = read_staging_mode(&dir).unwrap().unwrap();
        assert_ne!(conflict, ReleaseMode::Production);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn verification_report_gates_finalize() {
        let dir = temp_dir("gate");
        let record = VerificationRecord::failure("github", "v1.2.0", ReleaseMode::Rehearsal, "x".into());
        write_verification_report(&dir, &[record]).unwrap();
        assert!(!provider_verified(&dir, "github", "v1.2.0").unwrap());

        let mut facts = build_facts(&dir);
        facts.manifest_sha256 = Some("a".repeat(64));
        let result = ReadBackResult {
            provider: "github",
            artifact_sha256: vec![("update-manifest.json".into(), "b".repeat(64))],
            manifest_sha256_hex: "a".repeat(64),
            key_id: "c".repeat(64),
        };
        write_verification_report(
            &dir,
            &[VerificationRecord::from_result(&result, "v1.2.0", ReleaseMode::Rehearsal)],
        )
        .unwrap();
        assert!(provider_verified(&dir, "github", "v1.2.0").unwrap());
        // A different provider/tag does not satisfy the gate.
        assert!(!provider_verified(&dir, "gitee", "v1.2.0").unwrap());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sidecar_content_is_sha256sum_form() {
        let content = sidecar_content("pkg.zip", &"a".repeat(64));
        assert_eq!(content, format!("{}  pkg.zip\n", "a".repeat(64)));
    }
}
