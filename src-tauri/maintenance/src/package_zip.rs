//! ZIP structural validation and allowlist-bounded staging extraction
//! (Phase 2C-A; moved into the shared maintenance core in Phase 2C-B so the
//! helper re-validates the package through the same single implementation).
//!
//! Frozen rules (protocol v1, "UpdateManifest" package section): the archive
//! entry set must equal the compiled protocol package allowlist exactly —
//! currently the nine root files — with zero directories, zero nested paths,
//! no absolute paths, no parent traversal, no backslash aliases, no ADS/colon
//! forms, no trailing-dot/trailing-space aliases, no symlink/reparse-style
//! entries, no case-insensitive duplicate logical names, only explicitly
//! allowed compression methods, and bounded total uncompressed size. Entry
//! count ≤ 64, total expanded ≤ 512 MiB, each managed EXE ≤ 128 MiB,
//! expansion ratio ≤ 100:1 (all compiled limits).
//!
//! Two security properties stay distinct: the nine-entry allowlist is the
//! package **authenticity set** — every entry is validated as a package
//! member, yet only the two managed executables (the protocol-1 runtime
//! mutation set) are extracted to staging. The support files never enter
//! staging, replacement, or rollback.
//!
//! What is implementation filesystem policy rather than frozen protocol
//! text: the allowed compression methods (Stored and Deflate — the only
//! methods the product's own packaging uses), the per-entry ratio check, and
//! the Windows attribute inspection for link-style entries. Everything else
//! above is frozen.

use std::io::{Read, Write};

use desktop_todo_update_core::{EXPANDED_MAX_BYTES, MANAGED_FILE_MAX_BYTES};
use sha2::{Digest, Sha256};
/// Lowercase hex of a SHA-256 digest already computed.
fn digest_hex(digest: &[u8]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The exact compiled package root allowlist (protocol 1, currently nine
/// entries). The set itself — not the count — is the security property.
pub const PACKAGE_ROOT_ALLOWLIST: [&str; 9] = [
    "desktop-todo-widget.exe",
    "desktop-todo-maintenance.exe",
    "install.ps1",
    "uninstall.ps1",
    "README.md",
    "README_ZH.md",
    "LICENSE",
    "LICENSE_ZH.md",
    "THIRD_PARTY_NOTICES.md",
];

/// Compiled ZIP resource limits (protocol v1, "Encoding and validation").
pub const ZIP_MAX_ENTRIES: usize = 64;
pub const ZIP_MAX_EXPANDED_BYTES: u64 = EXPANDED_MAX_BYTES;
/// Maximum expansion ratio per entry (compressed → uncompressed).
pub const ZIP_MAX_RATIO: u64 = 100;

/// Why an archive was rejected. Every variant is a package-level failure —
/// the package is quarantined, never partially staged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveError {
    Malformed {
        detail: String,
    },
    TooManyEntries {
        count: usize,
        max: usize,
    },
    PathUnsafe {
        name: String,
    },
    EntryTypeUnsafe {
        name: String,
    },
    DuplicateEntry {
        name: String,
    },
    CompressionMethodNotAllowed {
        name: String,
        method: u16,
    },
    EntryTooLarge {
        name: String,
        size: u64,
    },
    ExpandedSizeExceeded {
        total: u64,
        max: u64,
    },
    CompressionRatioExceeded {
        name: String,
        ratio: u64,
    },
    AllowlistMismatch {
        missing: Vec<String>,
        unexpected: Vec<String>,
    },
    ExtractedSizeMismatch {
        file: String,
        declared: u64,
        actual: u64,
    },
    ExtractedHashMismatch {
        file: String,
        expected: String,
        actual: String,
    },
    Io {
        detail: String,
    },
}

/// The two staged managed executables, each cross-checked against the signed
/// manifest's `installFiles` size and SHA-256 after extraction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedExecutable {
    pub identity: String,
    pub filename: String,
    pub size: u64,
    pub sha256_hex: String,
}

/// Numeric protocol value for the archive compression method
/// (`Compression::Stored` = 0, `Compression::Deflate` = 8).
fn compression_method(method: zip::CompressionMethod) -> u16 {
    #[allow(deprecated)]
    match method {
        zip::CompressionMethod::Stored => 0,
        zip::CompressionMethod::Deflated => 8,
        other => other.to_u16(),
    }
}

/// Validate one archive entry name against the frozen path rules. The exact
/// allowlist equality below subsumes most path attacks (a traversal or ADS
/// name can never equal an allowlist entry), but each rule is checked
/// explicitly so the error taxonomy names the actual violation.
fn entry_name_is_safe(name: &str) -> Result<(), ArchiveError> {
    let reject = |name: &str| ArchiveError::PathUnsafe {
        name: name.to_string(),
    };
    if name.is_empty() || name.len() > 255 {
        return Err(reject(name));
    }
    if name.contains('\\')
        || name.contains(':') // ADS / drive prefixes
        || name.contains('\0')
        || name.contains("../")
        || name == ".."
        || name.starts_with('/')
        || name.starts_with("./")
        || name.split('/').any(|part| part == ".." || part == ".")
    {
        return Err(reject(name));
    }
    // Trailing dot / trailing space aliases (Windows normalization).
    if name.ends_with('.') || name.ends_with(' ') {
        return Err(reject(name));
    }
    // Reserved device names, with or without extension.
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
    ) {
        return Err(reject(name));
    }
    Ok(())
}

/// Inspect one archive entry's metadata and accumulate the expanded-size
/// budget. Called for every entry before any extraction happens.
struct EntryFacts<'a> {
    name: &'a str,
    is_dir: bool,
    compression: u16,
    compressed_size: u64,
    uncompressed_size: u64,
    is_symlink: bool,
}

#[allow(clippy::too_many_arguments)]
fn validate_entry(
    facts: EntryFacts<'_>,
    seen_lower: &mut Vec<String>,
    expanded_total: &mut u64,
) -> Result<(), ArchiveError> {
    let EntryFacts {
        name,
        is_dir,
        compression,
        compressed_size,
        uncompressed_size,
        is_symlink,
    } = facts;
    entry_name_is_safe(name)?;
    if is_dir {
        // Zero directory entries: root-level regular files only.
        return Err(ArchiveError::EntryTypeUnsafe {
            name: name.to_string(),
        });
    }
    // Link-style entries: the archive-declared symlink type (Unix mode bits
    // or the crate's own detection) is rejected — never followed.
    if is_symlink {
        return Err(ArchiveError::EntryTypeUnsafe {
            name: name.to_string(),
        });
    }
    // Only explicitly allowed compression methods (implementation policy:
    // Stored = 0, Deflate = 8 — the methods the product's own packaging uses).
    if compression != 0 && compression != 8 {
        return Err(ArchiveError::CompressionMethodNotAllowed {
            name: name.to_string(),
            method: compression,
        });
    }
    if uncompressed_size > MANAGED_FILE_MAX_BYTES {
        return Err(ArchiveError::EntryTooLarge {
            name: name.to_string(),
            size: uncompressed_size,
        });
    }
    // Per-entry expansion ratio (implementation policy): compressed == 0
    // with a non-empty payload is unrepresentable, so treat it as a bomb.
    if uncompressed_size > 0 {
        let ratio = uncompressed_size / compressed_size.max(1);
        if ratio > ZIP_MAX_RATIO {
            return Err(ArchiveError::CompressionRatioExceeded {
                name: name.to_string(),
                ratio,
            });
        }
    }
    // Case-insensitive duplicate logical names (frozen rule).
    let lower = name.to_ascii_lowercase();
    if seen_lower.contains(&lower) {
        return Err(ArchiveError::DuplicateEntry {
            name: name.to_string(),
        });
    }
    seen_lower.push(lower);

    *expanded_total = expanded_total.saturating_add(uncompressed_size);
    if *expanded_total > ZIP_MAX_EXPANDED_BYTES {
        return Err(ArchiveError::ExpandedSizeExceeded {
            total: *expanded_total,
            max: ZIP_MAX_EXPANDED_BYTES,
        });
    }
    Ok(())
}

/// Fully validate the archive metadata against the frozen package rules and
/// the exact compiled allowlist — before any entry is read.
pub fn validate_archive<R: std::io::Read + std::io::Seek>(
    reader: R,
) -> Result<zip::ZipArchive<R>, ArchiveError> {
    let mut archive = zip::ZipArchive::new(reader).map_err(|e| ArchiveError::Malformed {
        detail: e.to_string(),
    })?;
    if archive.len() > ZIP_MAX_ENTRIES {
        return Err(ArchiveError::TooManyEntries {
            count: archive.len(),
            max: ZIP_MAX_ENTRIES,
        });
    }

    let mut seen_lower: Vec<String> = Vec::new();
    let mut seen_exact: Vec<String> = Vec::new();
    let mut expanded_total: u64 = 0;
    for index in 0..archive.len() {
        let entry = archive
            .by_index_raw(index)
            .map_err(|e| ArchiveError::Malformed {
                detail: e.to_string(),
            })?;
        let name = entry.name().to_string();
        validate_entry(
            EntryFacts {
                name: &name,
                is_dir: entry.is_dir(),
                compression: compression_method(entry.compression()),
                compressed_size: entry.compressed_size(),
                uncompressed_size: entry.size(),
                is_symlink: entry.is_symlink(),
            },
            &mut seen_lower,
            &mut expanded_total,
        )?;
        seen_exact.push(name);
    }

    // Exact allowlist equality — byte-for-byte names, not fewer, not more,
    // no case aliases (a case-variant spelling is simply not an allowlist
    // name), no duplicates (an exact duplicate already failed above).
    let mut missing: Vec<String> = PACKAGE_ROOT_ALLOWLIST
        .iter()
        .filter(|allowed| !seen_exact.iter().any(|name| name == *allowed))
        .map(|allowed| allowed.to_string())
        .collect();
    let unexpected: Vec<String> = seen_exact
        .iter()
        .filter(|name| !PACKAGE_ROOT_ALLOWLIST.contains(&name.as_str()))
        .cloned()
        .collect();
    if !missing.is_empty() || !unexpected.is_empty() {
        missing.sort();
        return Err(ArchiveError::AllowlistMismatch {
            missing,
            unexpected,
        });
    }
    Ok(archive)
}

/// Extract only the two managed executables into `staged_dir`, streaming
/// each through SHA-256 and the declared size bound, and cross-check both
/// against the signed `installFiles` entries (expected: identity/filename/
/// size/sha256 from the verified manifest). Returns the staged facts.
pub fn extract_managed_executables<R: std::io::Read + std::io::Seek>(
    mut archive: zip::ZipArchive<R>,
    staged_dir: &std::path::Path,
    expected: &[(String, String, u64, String)], // (identity, filename, size, sha256)
) -> Result<Vec<StagedExecutable>, ArchiveError> {
    std::fs::create_dir_all(staged_dir).map_err(|e| ArchiveError::Io {
        detail: e.to_string(),
    })?;

    let mut staged = Vec::new();
    for (identity, filename, declared_size, declared_sha) in expected {
        // The exact allowlist validation above guarantees the entry exists
        // under exactly this name.
        let index =
            archive
                .index_for_name(filename)
                .ok_or_else(|| ArchiveError::AllowlistMismatch {
                    missing: vec![filename.clone()],
                    unexpected: Vec::new(),
                })?;
        let mut entry = archive
            .by_index(index)
            .map_err(|e| ArchiveError::Malformed {
                detail: e.to_string(),
            })?;

        let destination = staged_dir.join(filename);
        let temp = staged_dir.join(format!("{filename}.extracting"));
        let mut file = std::fs::File::create(&temp).map_err(|e| ArchiveError::Io {
            detail: e.to_string(),
        })?;
        let mut hasher = Sha256::new();
        let mut written: u64 = 0;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = entry.read(&mut buffer).map_err(|e| ArchiveError::Io {
                detail: e.to_string(),
            })?;
            if read == 0 {
                break;
            }
            written = written.saturating_add(read as u64);
            // Streaming bound: refuse mid-stream once the declared size is
            // exceeded (declared comes from the signed manifest).
            if written > *declared_size {
                let _ = std::fs::remove_file(&temp);
                return Err(ArchiveError::EntryTooLarge {
                    name: filename.clone(),
                    size: written,
                });
            }
            hasher.update(&buffer[..read]);
            file.write_all(&buffer[..read])
                .map_err(|e| ArchiveError::Io {
                    detail: e.to_string(),
                })?;
        }
        file.sync_all().map_err(|e| ArchiveError::Io {
            detail: e.to_string(),
        })?;
        drop(file);

        let actual_sha = digest_hex(&hasher.finalize());
        let actual_size = std::fs::metadata(&temp)
            .map_err(|e| ArchiveError::Io {
                detail: e.to_string(),
            })?
            .len();
        // Cross-check against the signed installFiles facts: package-level
        // ZIP hash correctness never substitutes for per-managed-file
        // verification.
        if actual_size != *declared_size {
            let _ = std::fs::remove_file(&temp);
            return Err(ArchiveError::ExtractedSizeMismatch {
                file: filename.clone(),
                declared: *declared_size,
                actual: actual_size,
            });
        }
        if actual_sha != *declared_sha {
            let _ = std::fs::remove_file(&temp);
            return Err(ArchiveError::ExtractedHashMismatch {
                file: filename.clone(),
                expected: declared_sha.clone(),
                actual: actual_sha,
            });
        }

        // Publish the verified file atomically over any prior bytes.
        crate::paths::move_replace(&temp, &destination).map_err(|e| {
            ArchiveError::Io {
                detail: e.to_string(),
            }
        })?;

        staged.push(StagedExecutable {
            identity: identity.clone(),
            filename: filename.clone(),
            size: actual_size,
            sha256_hex: actual_sha,
        });
    }
    Ok(staged)
}

/// Re-verify files already staged on disk (restart path): every staged
/// executable must still match its signed size and hash.
pub fn verify_staged_executables(
    staged_dir: &std::path::Path,
    expected: &[(String, String, u64, String)],
) -> Result<Vec<StagedExecutable>, ArchiveError> {
    let mut staged = Vec::new();
    for (identity, filename, declared_size, declared_sha) in expected {
        let path = staged_dir.join(filename);
        let bytes = std::fs::read(&path).map_err(|e| ArchiveError::Io {
            detail: format!("staged {} unreadable: {e}", filename),
        })?;
        if bytes.len() as u64 != *declared_size {
            return Err(ArchiveError::ExtractedSizeMismatch {
                file: filename.clone(),
                declared: *declared_size,
                actual: bytes.len() as u64,
            });
        }
        let hash = desktop_todo_update_core::sha256_hex(&bytes);
        if hash != *declared_sha {
            return Err(ArchiveError::ExtractedHashMismatch {
                file: filename.clone(),
                expected: declared_sha.clone(),
                actual: hash,
            });
        }
        staged.push(StagedExecutable {
            identity: identity.clone(),
            filename: filename.clone(),
            size: *declared_size,
            sha256_hex: hash,
        });
    }
    Ok(staged)
}
