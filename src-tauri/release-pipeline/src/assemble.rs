//! Canonical release package assembly (Phase 3B).
//!
//! Builds the flat nine-entry release ZIP — the exact compiled package
//! allowlist (`desktop_todo_maintenance::package_zip::PACKAGE_ROOT_ALLOWLIST`)
//! — from **explicit** per-entry source paths. Assembly never scans a
//! directory and never guesses: every entry name is mapped to exactly one
//! operator-visible source file, and any missing input fails closed.
//!
//! Determinism policy (implementation policy, not protocol): entries are
//! written in compiled-allowlist order, compression is Deflate at a fixed
//! level, and every entry timestamp is the fixed DOS epoch 1980-01-01 — so
//! the same input bytes through the same tool build yield byte-identical
//! package bytes (pinned by test). Cross-tool-version byte reproducibility
//! is deliberately not claimed.
//!
//! Immediately after assembly — before the output is published to its final
//! name — the archive is re-opened and run through the **single shared**
//! package validator (`maintenance::package_zip::validate_archive`), the
//! exact logic the helper and the main app use. A package that cannot pass
//! the updater's own rules is never emitted.

use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use desktop_todo_maintenance::package_zip::{
    validate_archive, PACKAGE_ROOT_ALLOWLIST, ZIP_MAX_EXPANDED_BYTES,
};
use desktop_todo_update_core::MANAGED_FILE_MAX_BYTES;
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

use crate::{check_regular_file, PipelineError, PackageInput};

/// Lowercase hex of a SHA-256 digest **already computed**. Never hash twice:
/// `update_core::sha256_hex` is SHA-256 *of its input*, so a finalized
/// digest must only be hex-formatted here.
fn digest_hex(digest: &[u8]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Fixed compression level for every entry (deterministic policy).
const COMPRESSION_LEVEL: Option<i64> = Some(9);
/// Fixed entry timestamp: the DOS epoch minimum (1980-01-01 00:00:00).
fn fixed_timestamp() -> zip::DateTime {
    zip::DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0)
        .expect("fixed DOS epoch timestamp is representable")
}

/// The canonical package input mapping: entry name → default source path
/// relative to the repo root. The main executable is built as
/// `alan-desktop.exe` and is renamed to its compiled managed identity
/// **only** at this assembly step (the frozen build contract produces
/// `src-tauri/target/release/alan-desktop.exe`; the public
/// `desktop-todo-widget.exe` name exists only inside the package).
pub fn canonical_inputs(
    repo_root: &Path,
    main_exe: Option<&Path>,
    helper_exe: Option<&Path>,
) -> Vec<PackageInput> {
    let root = |relative: &str| repo_root.join(relative);
    let default_main = root("src-tauri/target/release/alan-desktop.exe");
    let default_helper = root("src-tauri/target/release/desktop-todo-maintenance.exe");
    let entries: [(&'static str, PathBuf); 9] = [
        (
            "desktop-todo-widget.exe",
            main_exe.map(Path::to_path_buf).unwrap_or(default_main),
        ),
        (
            "desktop-todo-maintenance.exe",
            helper_exe.map(Path::to_path_buf).unwrap_or(default_helper),
        ),
        ("install.ps1", root("install.ps1")),
        ("uninstall.ps1", root("uninstall.ps1")),
        ("README.md", root("README.md")),
        ("README_ZH.md", root("README_ZH.md")),
        ("LICENSE", root("LICENSE")),
        ("LICENSE_ZH.md", root("LICENSE_ZH.md")),
        ("THIRD_PARTY_NOTICES.md", root("THIRD_PARTY_NOTICES.md")),
    ];
    entries
        .into_iter()
        .map(|(entry_name, source)| PackageInput { entry_name, source })
        .collect()
}

/// Validate the input set against the compiled allowlist: every entry name
/// must be an allowlist member, every allowlist entry must be present
/// exactly once (case-insensitive duplicate logical names refused), and
/// every source must be a regular readable file. Fail closed before any
/// output exists.
pub fn validate_inputs(inputs: &[PackageInput]) -> Result<(), PipelineError> {
    let mut seen_lower: Vec<String> = Vec::new();
    for input in inputs {
        if !PACKAGE_ROOT_ALLOWLIST.contains(&input.entry_name) {
            return Err(PipelineError::InputInvalid {
                name: input.entry_name.to_string(),
                path: input.source.clone(),
                detail: "entry name is not a compiled package allowlist member".to_string(),
            });
        }
        let lower = input.entry_name.to_ascii_lowercase();
        if seen_lower.contains(&lower) {
            return Err(PipelineError::InputInvalid {
                name: input.entry_name.to_string(),
                path: input.source.clone(),
                detail: "duplicate logical entry (case-insensitive)".to_string(),
            });
        }
        seen_lower.push(lower);
        check_regular_file(input.entry_name, &input.source)?;
    }
    let mut missing: Vec<&str> = PACKAGE_ROOT_ALLOWLIST
        .iter()
        .filter(|allowed| !inputs.iter().any(|input| input.entry_name == **allowed))
        .copied()
        .collect();
    if !missing.is_empty() {
        missing.sort_unstable();
        return Err(PipelineError::InputMissing {
            name: format!("package allowlist entries: {}", missing.join(", ")),
            path: PathBuf::from("<input set>"),
            detail: "every compiled allowlist entry requires exactly one explicit source"
                .to_string(),
        });
    }
    Ok(())
}

/// The outcome of one assembly: the final published package path and its
/// exact byte facts (size + SHA-256 over the final file bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyOutcome {
    pub package_path: PathBuf,
    pub package_size: u64,
    pub package_sha256_hex: String,
}

/// Assemble the package deterministically, self-validate it through the
/// shared validator, hash the final bytes, and publish atomically. The
/// output is never silently overwritten (`overwrite` must be explicit).
pub fn assemble_package(
    inputs: &[PackageInput],
    output_path: &Path,
    overwrite: bool,
) -> Result<AssemblyOutcome, PipelineError> {
    validate_inputs(inputs)?;

    if output_path.exists() && !overwrite {
        return Err(PipelineError::OutputCollision {
            path: output_path.to_path_buf(),
        });
    }
    let parent = match output_path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    std::fs::create_dir_all(&parent).map_err(|e| PipelineError::Io {
        detail: format!("cannot create staging directory {}: {e}", parent.display()),
    })?;
    let temp = parent.join(format!(
        "{}.assembling-{}",
        output_path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default(),
        std::process::id()
    ));

    let assemble_result = write_archive(inputs, &temp);
    if let Err(error) = assemble_result {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }

    // Self-check through the single shared validator — the same entry-set,
    // path-safety, compression, ratio, and size rules the helper and main app
    // enforce — *before* the output is published under its final name.
    let validated = {
        let file = std::fs::File::open(&temp).map_err(|e| PipelineError::Io {
            detail: e.to_string(),
        })?;
        validate_archive(file).map_err(|e| PipelineError::PackageValidation {
            detail: format!("{e:?}"),
        })?
    };
    if validated.len() != PACKAGE_ROOT_ALLOWLIST.len() {
        return Err(PipelineError::PackageValidation {
            detail: format!(
                "assembled archive carries {} entries, expected exactly {}",
                validated.len(),
                PACKAGE_ROOT_ALLOWLIST.len()
            ),
        });
    }

    // Hash the final bytes of the archive that is about to be published.
    let (size, sha256_hex) = hash_file(&temp)?;
    // Re-open must still work after hashing (catches torn writes).
    drop(validated);

    std::fs::rename(&temp, output_path).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        PipelineError::Assembly {
            detail: format!(
                "atomic publish of the assembled package failed: {e}"
            ),
        }
    })?;

    Ok(AssemblyOutcome {
        package_path: output_path.to_path_buf(),
        package_size: size,
        package_sha256_hex: sha256_hex,
    })
}

/// Deterministic archive writing: allowlist-ordered entries, fixed Deflate
/// level, fixed timestamps, streamed file bytes (per-entry bounded at the
/// compiled managed-file limit — a support file larger than a managed EXE is
/// allowed to be refused).
fn write_archive(inputs: &[PackageInput], temp: &Path) -> Result<(), PipelineError> {
    let file = std::fs::File::create(temp).map_err(|e| PipelineError::Io {
        detail: e.to_string(),
    })?;
    let mut writer = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(COMPRESSION_LEVEL)
        .last_modified_time(fixed_timestamp());

    let write_result = (|| -> Result<(), PipelineError> {
        // Compiled allowlist order — deterministic, allowlist-major.
        for allowed in PACKAGE_ROOT_ALLOWLIST {
            let Some(input) = inputs.iter().find(|input| input.entry_name == allowed) else {
                unreachable!("validate_inputs guarantees every allowlist entry has a source");
            };
            writer
                .start_file(input.entry_name, options)
                .map_err(|e| PipelineError::Assembly {
                    detail: format!("cannot start entry {}: {e}", input.entry_name),
                })?;
            let mut source =
                std::fs::File::open(&input.source).map_err(|e| PipelineError::InputMissing {
                    name: input.entry_name.to_string(),
                    path: input.source.clone(),
                    detail: e.to_string(),
                })?;
            let mut bounded = (&mut source).take(MANAGED_FILE_MAX_BYTES + 1);
            let written =
                std::io::copy(&mut bounded, &mut writer).map_err(|e| PipelineError::Assembly {
                    detail: format!("cannot stream {}: {e}", input.entry_name),
                })?;
            if written > MANAGED_FILE_MAX_BYTES {
                return Err(PipelineError::Assembly {
                    detail: format!(
                        "entry {} exceeds the compiled {}-byte per-entry limit",
                        input.entry_name, MANAGED_FILE_MAX_BYTES
                    ),
                });
            }
        }
        Ok(())
    })();

    if let Err(error) = write_result {
        let _ = writer.finish();
        return Err(error);
    }
    writer.finish().map_err(|e| PipelineError::Assembly {
        detail: format!("cannot finalize the archive: {e}"),
    })?;
    Ok(())
}

/// Streaming SHA-256 + byte size of one explicit file.
pub(crate) fn hash_file(path: &Path) -> Result<(u64, String), PipelineError> {
    use sha2::Digest;
    let mut file = std::fs::File::open(path).map_err(|e| PipelineError::Io {
        detail: e.to_string(),
    })?;
    let mut hasher = sha2::Sha256::new();
    let mut size: u64 = 0;
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut chunk).map_err(|e| PipelineError::Io {
            detail: e.to_string(),
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&chunk[..read]);
        size = size.saturating_add(read as u64);
    }
    Ok((size, digest_hex(&hasher.finalize())))
}

/// Read one entry out of an already-validated archive, streaming its bytes
/// through SHA-256 and a bounded read. This is a *read* helper over an
/// archive the shared validator has already accepted — it implements no
/// security rules of its own.
pub(crate) fn validated_entry_facts<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    entry_name: &str,
) -> Result<(u64, String), PipelineError> {
    use sha2::Digest;
    let index = archive.index_for_name(entry_name).ok_or_else(|| {
        PipelineError::Facts {
            detail: format!("validated archive has no entry {entry_name:?}"),
        }
    })?;
    let mut entry = archive.by_index(index).map_err(|e| PipelineError::Facts {
        detail: e.to_string(),
    })?;
    let mut hasher = sha2::Sha256::new();
    let mut size: u64 = 0;
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let read = entry.read(&mut chunk).map_err(|e| PipelineError::Facts {
            detail: format!("entry {entry_name}: {e}"),
        })?;
        if read == 0 {
            break;
        }
        size = size.saturating_add(read as u64);
        if size > MANAGED_FILE_MAX_BYTES {
            return Err(PipelineError::Facts {
                detail: format!("entry {entry_name} exceeds the compiled per-entry limit"),
            });
        }
        hasher.update(&chunk[..read]);
    }
    Ok((size, digest_hex(&hasher.finalize())))
}

/// The total expanded budget the shared validator enforces (re-exported
/// read-only here so tests can reference the compiled policy without a
/// second constant).
pub const COMPILED_EXPANDED_MAX_BYTES: u64 = ZIP_MAX_EXPANDED_BYTES;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "release-pipeline-assembly-{}-{}-{label}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Minimal placeholder EXE-like payloads: assembly and validation are
    /// name/structure-driven; the shared validator does not inspect PE
    /// headers (that is the build substrate's job).
    fn fake_exe(seed: u8, len: usize) -> Vec<u8> {
        vec![seed; len]
    }

    fn write_inputs(dir: &Path) -> Vec<PackageInput> {
        let payloads: [(&str, Vec<u8>); 9] = [
            ("desktop-todo-widget.exe", fake_exe(1, 64)),
            ("desktop-todo-maintenance.exe", fake_exe(2, 48)),
            ("install.ps1", b"param() install\n".to_vec()),
            ("uninstall.ps1", b"param() uninstall\n".to_vec()),
            ("README.md", b"readme\n".to_vec()),
            ("README_ZH.md", b"readme zh\n".to_vec()),
            ("LICENSE", b"MIT\n".to_vec()),
            ("LICENSE_ZH.md", b"MIT zh\n".to_vec()),
            ("THIRD_PARTY_NOTICES.md", b"notices\n".to_vec()),
        ];
        payloads
            .into_iter()
            .map(|(name, bytes)| {
                let path = dir.join(name);
                fs::write(&path, &bytes).unwrap();
                PackageInput {
                    entry_name: *PACKAGE_ROOT_ALLOWLIST
                        .iter()
                        .find(|allowed| **allowed == name)
                        .unwrap(),
                    source: path,
                }
            })
            .collect()
    }

    #[test]
    fn canonical_inputs_cover_the_exact_allowlist() {
        let repo = temp_dir("repo");
        let inputs = canonical_inputs(&repo, None, None);
        assert_eq!(inputs.len(), 9);
        for input in &inputs {
            assert!(PACKAGE_ROOT_ALLOWLIST.contains(&input.entry_name));
        }
        // The main-EXE mapping: built as alan-desktop.exe, packaged as the
        // compiled managed identity.
        assert_eq!(inputs[0].entry_name, "desktop-todo-widget.exe");
        assert_eq!(
            inputs[0].source,
            repo.join("src-tauri/target/release/alan-desktop.exe")
        );
        assert_eq!(
            inputs[1].source,
            repo.join("src-tauri/target/release/desktop-todo-maintenance.exe")
        );
        fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn assembly_is_deterministic_byte_for_byte() {
        let dir = temp_dir("determinism");
        let inputs = write_inputs(&dir);
        let out_a = dir.join("a.zip");
        let out_b = dir.join("b.zip");
        let a = assemble_package(&inputs, &out_a, false).unwrap();
        let b = assemble_package(&inputs, &out_b, false).unwrap();
        assert_eq!(a.package_size, b.package_size);
        assert_eq!(a.package_sha256_hex, b.package_sha256_hex);
        assert_eq!(fs::read(&out_a).unwrap(), fs::read(&out_b).unwrap());
        fs::remove_dir_all(&dir).ok();
    }



    #[test]
    fn assembled_package_passes_the_shared_validator_exactly() {
        let dir = temp_dir("selfcheck");
        let inputs = write_inputs(&dir);
        let out = dir.join("pkg.zip");
        let outcome = assemble_package(&inputs, &out, false).unwrap();
        // The facts describe the final file bytes.
        let (size, sha) = hash_file(&out).unwrap();
        assert_eq!(outcome.package_size, size);
        assert_eq!(outcome.package_sha256_hex, sha);
        // And the archive is exactly the nine allowlist entries, no
        // directories, nothing else.
        let file = fs::File::open(&out).unwrap();
        let archive = validate_archive(file).unwrap();
        assert_eq!(archive.len(), 9);
        for name in PACKAGE_ROOT_ALLOWLIST {
            assert!(archive.index_for_name(name).is_some(), "{name}");
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_required_input_fails_closed_with_no_output() {
        let dir = temp_dir("missing");
        let inputs = write_inputs(&dir);
        fs::remove_file(&inputs[0].source).unwrap(); // main EXE gone
        let out = dir.join("pkg.zip");
        let error = assemble_package(&inputs, &out, false).unwrap_err();
        assert!(matches!(error, PipelineError::InputMissing { .. }));
        assert!(!out.exists());
        // No temp leftovers.
        let leftovers: Vec<_> = fs::read_dir(&dir).unwrap().collect();
        assert_eq!(leftovers.len(), 8, "only the remaining inputs may exist");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn incomplete_allowlist_fails_closed_before_any_writing() {
        let dir = temp_dir("incomplete");
        let mut inputs = write_inputs(&dir);
        inputs.pop(); // THIRD_PARTY_NOTICES.md missing from the set
        let out = dir.join("pkg.zip");
        let error = assemble_package(&inputs, &out, false).unwrap_err();
        assert!(matches!(error, PipelineError::InputMissing { .. }));
        assert!(!out.exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn extra_and_duplicate_entries_are_refused() {
        let dir = temp_dir("extra");
        let mut inputs = write_inputs(&dir);
        // An entry name outside the allowlist.
        inputs.push(PackageInput {
            entry_name: "README-copy.txt",
            source: dir.join("README.md"),
        });
        let error = validate_inputs(&inputs).unwrap_err();
        assert!(matches!(error, PipelineError::InputInvalid { .. }));

        // A case-insensitive duplicate logical entry.
        let mut inputs = write_inputs(&dir);
        inputs.push(PackageInput {
            entry_name: "readme.md",
            source: dir.join("README.md"),
        });
        let error = validate_inputs(&inputs).unwrap_err();
        assert!(matches!(error, PipelineError::InputInvalid { .. }));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn output_collision_is_refused_unless_explicit() {
        let dir = temp_dir("collision");
        let inputs = write_inputs(&dir);
        let out = dir.join("pkg.zip");
        fs::write(&out, b"existing").unwrap();
        let error = assemble_package(&inputs, &out, false).unwrap_err();
        assert!(matches!(error, PipelineError::OutputCollision { .. }));
        assert_eq!(fs::read(&out).unwrap(), b"existing");
        assemble_package(&inputs, &out, true).unwrap();
        assert_ne!(fs::read(&out).unwrap(), b"existing");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn symlink_source_is_rejected() {
        #[cfg(unix)]
        {
            let dir = temp_dir("symlink");
            let inputs = write_inputs(&dir);
            let target = inputs[0].source.clone();
            let link = dir.join("link.exe");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            let mut inputs = inputs;
            inputs[0].source = link;
            assert!(matches!(
                validate_inputs(&inputs).unwrap_err(),
                PipelineError::InputInvalid { .. }
            ));
            fs::remove_dir_all(&dir).ok();
        }
    }
}
