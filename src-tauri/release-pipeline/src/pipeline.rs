//! The prepare orchestration (Phase 3B): assemble → facts → manifest →
//! sign → sidecar → reports, with explicit collision policy, mode
//! isolation, and a dry-run that writes nothing.

use std::path::PathBuf;

use crate::assemble::{assemble_package, canonical_inputs, AssemblyOutcome};
use crate::facts::{compute_facts, ReleaseFacts};
use crate::manifest::{generate_manifest_bytes, ManifestInput};
use crate::publish::{tag_for_version, ArtifactSet};
use crate::report::{
    read_staging_mode, sidecar_content, write_facts_json, write_release_report, ReleaseReport,
};
use crate::signing::SigningOutcome;
use crate::{ReleaseMode, PipelineError};

/// Everything the prepare step needs. `mode` selects the signing path;
/// rehearsal additionally requires the test key/trust-public paths.
#[derive(Debug, Clone)]
pub struct PrepareOptions {
    pub staging: PathBuf,
    pub repo_root: PathBuf,
    pub main_exe: Option<PathBuf>,
    pub helper_exe: Option<PathBuf>,
    pub version: String,
    pub published_at: String,
    pub notes: String,
    pub mode: ReleaseMode,
    /// Rehearsal only: raw 32-byte TEST seed file.
    pub key_path: Option<PathBuf>,
    /// Rehearsal only: raw 32-byte TEST public key file.
    pub trust_public_path: Option<PathBuf>,
    /// Source commit SHA recorded for provenance (display fact only).
    pub source_commit: Option<String>,
    pub overwrite: bool,
}

/// The prepared artifact set plus facts (the handoff to publish/read-back).
#[derive(Debug, Clone)]
pub struct PrepareOutcome {
    pub assembly: AssemblyOutcome,
    pub facts: ReleaseFacts,
    pub manifest_bytes: Vec<u8>,
    pub signing: SigningOutcome,
    pub sidecar_path: PathBuf,
    pub manifest_sha256_hex: String,
}

/// Validate the operator release identity up front: frozen version grammar
/// and the tag convention (`v{version}`).
pub fn validate_release_identity(version: &str, tag: Option<&str>) -> Result<String, PipelineError> {
    let expected_tag = tag_for_version(version)?;
    if let Some(tag) = tag {
        if tag != expected_tag {
            return Err(PipelineError::ReleaseIdentity {
                detail: format!(
                    "release tag {tag:?} contradicts the manifest version ({expected_tag:?})"
                ),
            });
        }
    }
    Ok(expected_tag)
}

/// Run the full local prepare. In `dry_run` nothing is written: the plan is
/// computed and returned as a display string.
pub fn prepare(options: &PrepareOptions, dry_run: bool) -> Result<String, PipelineError> {
    let tag = validate_release_identity(&options.version, None)?;
    if !dry_run {
        // Dry run performs no signing, so it needs no key material; the
        // full gates below run only when actually executing.
        match options.mode {
            ReleaseMode::Rehearsal => {
                if options.key_path.is_none() || options.trust_public_path.is_none() {
                    return Err(PipelineError::Usage(
                        "rehearsal mode requires --key <test seed> and --trust-public <test public key>"
                            .to_string(),
                    ));
                }
            }
            ReleaseMode::Production => {
                // The production signing key still comes from the operator's
                // local seed file; the compiled production trust gate decides
                // whether that key may sign at all.
                if options.key_path.is_none() {
                    return Err(PipelineError::Usage(
                        "production mode requires --key <operator seed file> (the compiled \
                         production trust gate refuses unprovisioned keys)"
                            .to_string(),
                    ));
                }
                if options.trust_public_path.is_some() {
                    return Err(PipelineError::Usage(
                        "--trust-public is rehearsal-only; production verification always uses the \
                         compiled production trust store"
                            .to_string(),
                    ));
                }
            }
        }
    }

    // Mode isolation: an existing staging root never switches modes.
    if let Some(existing) = read_staging_mode(&options.staging)? {
        if existing != options.mode {
            return Err(PipelineError::ModeConflict {
                detail: format!(
                    "staging root {} was prepared in {:?} mode; refusing to reuse it in {:?} mode",
                    options.staging.display(),
                    existing,
                    options.mode
                ),
            });
        }
    }

    let inputs = canonical_inputs(
        &options.repo_root,
        options.main_exe.as_deref(),
        options.helper_exe.as_deref(),
    );

    if dry_run {
        let mut plan = String::new();
        plan.push_str(&format!("release pipeline dry run (no output written)\n"));
        plan.push_str(&format!("  mode:            {}\n", options.mode.as_str()));
        plan.push_str(&format!("  target version:  {}\n", options.version));
        plan.push_str(&format!("  release tag:     {tag}\n"));
        plan.push_str(&format!("  staging root:    {}\n", options.staging.display()));
        plan.push_str("  package inputs (exact allowlist order):\n");
        for input in &inputs {
            plan.push_str(&format!("    {} <- {}\n", input.entry_name, input.source.display()));
        }
        plan.push_str(&format!(
            "  package output:  {}\n",
            options
                .staging
                .join(format!("desktop-todo-widget-v{}-windows-x64.zip", options.version))
                .display()
        ));
        plan.push_str(&format!(
            "  manifest output: {}\n",
            options.staging.join(crate::MANIFEST_ASSET_NAME).display()
        ));
        plan.push_str(&format!(
            "  envelope output: {}\n",
            options.staging.join(crate::ENVELOPE_ASSET_NAME).display()
        ));
        plan.push_str(&format!(
            "  sidecar output:  {}\n",
            options
                .staging
                .join(format!(
                    "desktop-todo-widget-v{}-windows-x64.zip{}",
                    options.version,
                    crate::SIDECAR_SUFFIX
                ))
                .display()
        ));
        if options.mode == ReleaseMode::Rehearsal {
            plan.push_str("  signing:         desktop-todo-release-signer with an injected TEST trust store\n");
        } else {
            plan.push_str("  signing:         desktop-todo-release-signer production gate (compiled production trust store; fails closed while unprovisioned)\n");
        }        plan.push_str("  provider publish: not performed by prepare; run `publish` / `read-back` separately\n");
        return Ok(plan);
    }

    // 1. Assemble + shared-validator self-check.
    let package_path = options.staging.join(format!(
        "desktop-todo-widget-v{}-windows-x64.zip",
        options.version
    ));
    let assembly = assemble_package(&inputs, &package_path, options.overwrite)?;

    // 2. Facts from the final package bytes.
    let mut facts = compute_facts(&package_path, options.mode, &options.version)?;

    // 3. Deterministic manifest bytes (validated before anything is signed).
    let manifest_input = ManifestInput {
        version: options.version.clone(),
        published_at: options.published_at.clone(),
        notes: options.notes.clone(),
    };
    let manifest_bytes = generate_manifest_bytes(&facts, &manifest_input)?;

    // 4. Sign the exact generated bytes (production gate or rehearsal
    //    injection — never anything in between).
    let signing = match options.mode {
        ReleaseMode::Production => crate::signing::sign_production(
            &manifest_bytes,
            options.key_path.as_ref().expect("checked above"),
        )?,
        ReleaseMode::Rehearsal => crate::signing::sign_rehearsal(
            &manifest_bytes,
            options.key_path.as_ref().expect("checked above"),
            options.trust_public_path.as_ref().expect("checked above"),
        )?,
    };
    if signing.version != options.version {
        return Err(PipelineError::ModeConflict {
            detail: format!(
                "signer verified version {} disagrees with the requested target {}",
                signing.version, options.version
            ),
        });
    }

    // 5. Publish the artifacts to the staging root (atomic, explicit
    //    overwrite policy). Manifest first, then envelope, then sidecar.
    let manifest_path = options.staging.join(crate::MANIFEST_ASSET_NAME);
    crate::write_output_atomic(&manifest_path, &manifest_bytes, options.overwrite)?;
    let envelope_path = options.staging.join(crate::ENVELOPE_ASSET_NAME);
    crate::write_output_atomic(&envelope_path, &signing.envelope, options.overwrite)?;
    facts.manifest_sha256 = Some(signing.manifest_sha256_hex.clone());
    facts.envelope_key_id = Some(signing.key_id.clone());
    let sidecar_path = options.staging.join(format!(
        "{}{}",
        facts.package_filename,
        crate::SIDECAR_SUFFIX
    ));
    crate::write_output_atomic(
        &sidecar_path,
        sidecar_content(&facts.package_filename, &facts.package_sha256_hex).as_bytes(),
        options.overwrite,
    )?;

    // 6. Reports.
    write_facts_json(&options.staging, &facts)?;
    let report = ReleaseReport {
        mode: options.mode,
        target_version: options.version.clone(),
        source_commit: options.source_commit.clone(),
        artifact_filenames: ArtifactSet::from_staging(&options.staging, &facts)
            .ordered()
            .into_iter()
            .map(|(name, _)| name)
            .collect(),
        provider_targets: vec![crate::publish::PRODUCTION_REPO.to_string()],
        staging_root: options.staging.display().to_string(),
    };
    write_release_report(&options.staging, &report)?;

    Ok(format!(
        "prepared {} release v{} in {}\n  package:  {}\n  manifest: {}\n  envelope: {} (keyId {})\n  sidecar:  {}\n  mode:     {}",
        options.mode.as_str(),
        options.version,
        options.staging.display(),
        assembly.package_path.display(),
        manifest_path.display(),
        envelope_path.display(),
        signing.key_id,
        sidecar_path.display(),
        options.mode.as_str(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "release-pipeline-prepare-{}-{}-{label}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Build a fake repo root carrying the support files and the two built
    /// EXEs at the canonical paths.
    fn fake_repo(label: &str) -> (PathBuf, PathBuf) {
        let repo = temp_dir(label);
        fs::create_dir_all(repo.join("src-tauri/target/release")).unwrap();
        let names = [
            ("install.ps1", b"install\n".to_vec()),
            ("uninstall.ps1", b"uninstall\n".to_vec()),
            ("README.md", b"r\n".to_vec()),
            ("README_ZH.md", b"rz\n".to_vec()),
            ("LICENSE", b"M\n".to_vec()),
            ("LICENSE_ZH.md", b"Mz\n".to_vec()),
            ("THIRD_PARTY_NOTICES.md", b"n\n".to_vec()),
        ];
        for (name, bytes) in names {
            fs::write(repo.join(name), bytes).unwrap();
        }
        fs::write(
            repo.join("src-tauri/target/release/alan-desktop.exe"),
            vec![1u8; 128],
        )
        .unwrap();
        fs::write(
            repo.join("src-tauri/target/release/desktop-todo-maintenance.exe"),
            vec![2u8; 96],
        )
        .unwrap();
        let staging = repo.join("staging");
        fs::create_dir_all(&staging).unwrap();
        (repo, staging)
    }

    fn rehearsal_options(repo: &Path, staging: &Path) -> PrepareOptions {
        let key = repo.join("test.seed");
        let public = repo.join("test.pub");
        let _ = fs::remove_file(&key);
        let _ = fs::remove_file(&public);
        desktop_todo_release_signer::generate_keypair(&key, &public).unwrap();
        PrepareOptions {
            staging: staging.to_path_buf(),
            repo_root: repo.to_path_buf(),
            main_exe: None,
            helper_exe: None,
            version: "1.2.0".to_string(),
            published_at: "2026-10-02T00:00:00Z".to_string(),
            notes: "Application lifecycle management.".to_string(),
            mode: ReleaseMode::Rehearsal,
            key_path: Some(key),
            trust_public_path: Some(public),
            source_commit: Some("258d660".to_string()),
            overwrite: false,
        }
    }

    #[test]
    fn dry_run_writes_nothing() {
        let (repo, staging) = fake_repo("dryrun");
        let options = rehearsal_options(&repo, &staging);
        let before = fs::read_dir(&staging).unwrap().count();
        let plan = prepare(&options, true).unwrap();
        assert!(plan.contains("dry run"));
        assert!(plan.contains("alan-desktop.exe"), "plan shows the exact build artifact mapping");
        let after = fs::read_dir(&staging).unwrap().count();
        assert_eq!(before, after, "dry run must not write anything");
        fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn full_rehearsal_prepare_produces_the_complete_artifact_set() {
        let (repo, staging) = fake_repo("full");
        let options = rehearsal_options(&repo, &staging);
        prepare(&options, false).unwrap();

        // The exact artifact set.
        let package = staging.join("desktop-todo-widget-v1.2.0-windows-x64.zip");
        let manifest = staging.join(crate::MANIFEST_ASSET_NAME);
        let envelope = staging.join(crate::ENVELOPE_ASSET_NAME);
        let sidecar = staging.join("desktop-todo-widget-v1.2.0-windows-x64.zip.sha256");
        for path in [&package, &manifest, &envelope, &sidecar] {
            assert!(path.exists(), "{} missing", path.display());
        }
        // facts.json records the rehearsal mode and the key id.
        let facts_text = fs::read_to_string(staging.join("facts.json")).unwrap();
        assert!(facts_text.contains(r#""mode": "rehearsal""#));
        assert!(facts_text.contains("envelopeKeyId"));
        // The sidecar is sha256sum-form.
        let sidecar_text = fs::read_to_string(&sidecar).unwrap();
        // sha256sum form: 64 lowercase hex, two spaces, filename, newline
        let (hex, name) = sidecar_text.split_once("  ").unwrap();
        assert_eq!(hex.len(), 64);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(name, "desktop-todo-widget-v1.2.0-windows-x64.zip
");
        // report.json records the commit.
        let report_text = fs::read_to_string(staging.join("report.json")).unwrap();
        assert!(report_text.contains("258d660"));
        fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn production_prepare_fails_closed_on_the_empty_store_but_never_writes() {
        let (repo, staging) = fake_repo("prod");
        // Production mode with a perfectly valid (but unprovisioned) key:
        // the compiled production trust gate must refuse it — exit 20 — and
        // no signed artifact may appear in staging.
        let key = repo.join("prod-test.seed");
        let public = repo.join("prod-test.pub");
        desktop_todo_release_signer::generate_keypair(&key, &public).unwrap();
        let options = PrepareOptions {
            mode: ReleaseMode::Production,
            key_path: Some(key),
            trust_public_path: None,
            ..rehearsal_options(&repo, &staging)
        };
        let error = prepare(&options, false).unwrap_err();
        assert!(matches!(error, PipelineError::Signing { .. }), "{error}");
        assert_eq!(error.exit_code(), 20);
        // Nothing was published into staging (the package file is written
        // before signing, so remove it to check the signed artifacts are
        // absent — assembly itself is not trust-sensitive).
        assert!(!staging.join(crate::MANIFEST_ASSET_NAME).exists());
        assert!(!staging.join(crate::ENVELOPE_ASSET_NAME).exists());
        assert!(!staging.join("facts.json").exists());
        fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn staging_mode_never_switches_silently() {
        let (repo, staging) = fake_repo("modeconflict");
        let options = rehearsal_options(&repo, &staging);
        prepare(&options, false).unwrap();
        let key = repo.join("prod-test.seed");
        let public = repo.join("prod-test.pub");
        desktop_todo_release_signer::generate_keypair(&key, &public).unwrap();
        let mut production = rehearsal_options(&repo, &staging);
        production.mode = ReleaseMode::Production;
        production.key_path = Some(key);
        production.trust_public_path = None;
        let error = prepare(&production, false).unwrap_err();
        assert!(matches!(error, PipelineError::ModeConflict { .. }));
        fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn tag_convention_is_v_plus_version() {
        assert_eq!(validate_release_identity("1.2.0", Some("v1.2.0")).unwrap(), "v1.2.0");
        let error = validate_release_identity("1.2.0", Some("release-1.2.0")).unwrap_err();
        assert!(matches!(error, PipelineError::ReleaseIdentity { .. }));
        assert!(validate_release_identity("1.2.0-beta", None).is_err());
    }
}
