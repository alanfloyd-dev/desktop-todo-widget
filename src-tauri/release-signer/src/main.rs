//! Offline release-signing CLI for Maintenance Protocol 1 (Phase 3A).
//!
//! Operator tooling: offline, no HTTP, no provider API, no Tauri, no
//! SQLite, no async runtime. The manifest is signed as its **exact raw
//! bytes** — the tool never reserializes, pretty-prints, canonicalizes, or
//! normalizes the signed document, and every sign operation ends in a
//! mandatory production-equivalent self-verification round-trip before any
//! output is published.
//!
//! Exit codes are typed (see `ToolError::exit_code`); every failure exits
//! non-zero. Private key material is never written to stdout/stderr.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use desktop_todo_release_signer as signer;
use desktop_todo_update_core::{ENVELOPE_MAX_BYTES, MANIFEST_MAX_BYTES};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // Error Display values are key-material-free by construction
            // (lib contract, pinned by tests).
            eprintln!("error: {error}");
            ExitCode::from(error.exit_code() as u8)
        }
    }
}

struct Flags {
    values: std::collections::HashMap<String, PathBuf>,
    switches: std::collections::HashSet<String>,
}

fn parse_flags(
    args: &[String],
    spec: &[&str],
    switches: &[&str],
) -> Result<Flags, signer::ToolError> {
    let usage = |detail: String| Err(signer::ToolError::Usage(detail));
    let mut values = std::collections::HashMap::new();
    let mut parsed_switches = std::collections::HashSet::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let Some(name) = arg.strip_prefix("--") else {
            return usage(format!("unexpected positional argument {arg:?}"));
        };
        if switches.contains(&name) {
            if parsed_switches.contains(name) {
                return usage(format!("flag --{name} given twice"));
            }
            parsed_switches.insert(name.to_string());
            index += 1;
            continue;
        }
        if !spec.contains(&name) {
            return usage(format!("unknown flag --{name}"));
        }
        let Some(value) = args.get(index + 1) else {
            return usage(format!("flag --{name} requires a path value"));
        };
        if value.starts_with("--") {
            return usage(format!("flag --{name} requires a path value"));
        }
        if values.contains_key(name) {
            return usage(format!("flag --{name} given twice"));
        }
        values.insert(name.to_string(), PathBuf::from(value));
        index += 2;
    }
    Ok(Flags {
        values,
        switches: parsed_switches,
    })
}

impl Flags {
    fn path(&self, name: &str) -> Result<&Path, signer::ToolError> {
        self.values
            .get(name)
            .map(|value| value.as_path())
            .ok_or_else(|| signer::ToolError::Usage(format!("missing required flag --{name}")))
    }
    fn switch(&self, name: &str) -> bool {
        self.switches.contains(name)
    }
}

fn run(args: &[String]) -> Result<(), signer::ToolError> {
    let Some(command) = args.first() else {
        return usage();
    };
    let rest = &args[1..];
    match command.as_str() {
        "generate-keypair" => generate_keypair(rest),
        "key-id" => key_id(rest),
        "provision-entry" => provision_entry(rest),
        "sign" => sign(rest),
        "verify" => verify(rest),
        "package-facts" => package_facts(rest),
        "--help" | "-h" | "help" => usage(),
        other => {
            let _ = other;
            usage()
        }
    }
}

fn usage() -> Result<(), signer::ToolError> {
    let text = r#"offline release signer for Maintenance Protocol 1 (Phase 3A)

USAGE:
  desktop-todo-release-signer generate-keypair --seed-out <path> --public-out <path>
  desktop-todo-release-signer key-id (--seed <path> | --public <path>)
  desktop-todo-release-signer provision-entry --public <path>
  desktop-todo-release-signer sign --manifest <path> --key <path> [--out <path>] [--overwrite]
      (the signing key must already be provisioned in the compiled production trust store)
  desktop-todo-release-signer verify --manifest <path> --envelope <path> (--seed <path> | --public <path>)
  desktop-todo-release-signer package-facts --package <path> [--main-exe <path>] [--helper-exe <path>]

Key files are raw 32-byte local files (no other encoding is accepted).
The signature always covers the exact raw manifest bytes.
See docs/release-signing.md for the operator workflow.
"#;
    println!("{text}");
    Ok(())
}

/// Resolve a key identity from `--seed <path>` or `--public <path>`:
/// exactly one must be present. Returns the verifying key.
fn resolve_key_identity(flags: &Flags) -> Result<ed25519_dalek::VerifyingKey, signer::ToolError> {
    let seed = flags.values.get("seed");
    let public = flags.values.get("public");
    match (seed, public) {
        (Some(_), Some(_)) => Err(signer::ToolError::Usage(
            "pass exactly one of --seed / --public".to_string(),
        )),
        (Some(path), None) => Ok(signer::load_signing_key(path)?.verifying_key()),
        (None, Some(path)) => signer::load_public_key(path),
        (None, None) => Err(signer::ToolError::Usage(
            "pass exactly one of --seed / --public".to_string(),
        )),
    }
}

fn generate_keypair(args: &[String]) -> Result<(), signer::ToolError> {
    let flags = parse_flags(args, &["seed-out", "public-out"], &[])?;
    let seed_out = flags.path("seed-out")?;
    let public_out = flags.path("public-out")?;
    if seed_out == public_out {
        return Err(signer::ToolError::Usage(
            "--seed-out and --public-out must be different files".to_string(),
        ));
    }
    let key_id = signer::generate_keypair(seed_out, public_out)?;
    println!(
        "{}",
        serde_json::json!({
            "generated": true,
            "keyId": key_id,
            "seedOut": seed_out.display().to_string(),
            "publicOut": public_out.display().to_string(),
            "note": "store the seed offline; only the public key may enter the repository"
        })
    );
    Ok(())
}

fn key_id(args: &[String]) -> Result<(), signer::ToolError> {
    let flags = parse_flags(args, &["seed", "public"], &[])?;
    let verifying = resolve_key_identity(&flags)?;
    println!(
        "{}",
        serde_json::json!({
            "keyId": signer::key_id_of(&verifying)
        })
    );
    Ok(())
}

/// Print the exact provisioning table entry for a public key. The operator
/// reviews and pastes it into `update-core/src/production_keys.rs`; the
/// consistency tests then pin the entry.
fn provision_entry(args: &[String]) -> Result<(), signer::ToolError> {
    let flags = parse_flags(args, &["public"], &[])?;
    let verifying = signer::load_public_key(flags.path("public")?)?;
    let raw = verifying.to_bytes();
    let key_id = signer::key_id_of(&verifying);
    let mut bytes = String::new();
    for line in raw.chunks(8) {
        bytes.push_str("        ");
        bytes.push_str(
            &line
                .iter()
                .map(|byte| format!("0x{byte:02x}"))
                .collect::<Vec<_>>()
                .join(", "),
        );
        bytes.push_str(",\n");
    }
    println!(
        "ProvisionedKey {{\n    declared_key_id: \"{key_id}\",\n    raw: [\n{bytes}    ],\n}},"
    );
    Ok(())
}

fn sign(args: &[String]) -> Result<(), signer::ToolError> {
    let flags = parse_flags(args, &["manifest", "key", "out"], &["overwrite"])?;
    let manifest_path = flags.path("manifest")?;
    let key_path = flags.path("key")?;
    let overwrite = flags.switch("overwrite");

    // The exact raw bytes as they will be served — never a parsed object,
    // never a reserialization. The read is bounded at the compiled limit.
    let manifest_bytes = signer::read_bounded(manifest_path, MANIFEST_MAX_BYTES)?;
    let key = signer::load_signing_key(key_path)?;
    let outcome = signer::sign_manifest(&manifest_bytes, &key)?;

    let out = flags
        .values
        .get("out")
        .cloned()
        .unwrap_or_else(|| manifest_path.with_file_name(signer::ENVELOPE_FILENAME));
    signer::write_atomic(&out, &outcome.envelope, overwrite)?;

    println!(
        "{}",
        serde_json::json!({
            "signed": true,
            "envelope": out.display().to_string(),
            "keyId": outcome.key_id,
            "manifestSha256": outcome.manifest_sha256,
            "version": outcome.version,
            "package": {
                "filename": outcome.package_filename,
                "size": outcome.package_size,
                "sha256": outcome.package_sha256,
            },
            "selfVerified": true,
        })
    );
    Ok(())
}

fn verify(args: &[String]) -> Result<(), signer::ToolError> {
    let flags = parse_flags(args, &["manifest", "envelope", "seed", "public"], &[])?;
    let manifest_path = flags.path("manifest")?;
    let envelope_path = flags.path("envelope")?;
    let verifying = resolve_key_identity(&flags)?;

    let manifest_bytes = signer::read_bounded(manifest_path, MANIFEST_MAX_BYTES)?;
    let envelope_bytes = signer::read_bounded(envelope_path, ENVELOPE_MAX_BYTES)?;
    let trust = signer::trust_from_public_key(&verifying)?;
    let facts = signer::verify_envelope(&trust, &envelope_bytes, &manifest_bytes)?;
    // Cross-check against the compiled production trust store: the operator
    // sees whether the key that verified this envelope is the actual
    // compiled authority (only `true` pairs are publishable).
    let provisioned = desktop_todo_update_core::production_key_ids().contains(&facts.key_id);
    println!(
        "{}",
        serde_json::json!({
            "verified": true,
            "keyId": facts.key_id,
            "keyIdProvisioned": provisioned,
            "manifestSha256": facts.manifest_sha256,
            "version": facts.version,
            "package": {
                "filename": facts.package_filename,
                "size": facts.package_size,
                "sha256": facts.package_sha256,
            },
        })
    );
    Ok(())
}

fn package_facts(args: &[String]) -> Result<(), signer::ToolError> {
    let flags = parse_flags(args, &["package", "main-exe", "helper-exe"], &[])?;
    // Both installFiles entries must be present together: the manifest
    // requires exactly the two fixed identities, so partial facts would
    // invite a half-authored document.
    if flags.values.contains_key("main-exe") ^ flags.values.contains_key("helper-exe") {
        return Err(signer::ToolError::Usage(
            "pass --main-exe and --helper-exe together (the manifest requires both identities)"
                .to_string(),
        ));
    }
    let package = flags.path("package")?;
    let (size, sha256) = signer::file_facts(package)?;
    let mut report = serde_json::json!({
        "package": {
            "path": package.display().to_string(),
            "size": size,
            "sha256": sha256,
        }
    });
    if let (Some(main), Some(helper)) =
        (flags.values.get("main-exe"), flags.values.get("helper-exe"))
    {
        let (main_size, main_sha256) = signer::file_facts(main)?;
        let (helper_size, helper_sha256) = signer::file_facts(helper)?;
        report = serde_json::json!({
            "package": {
                "path": package.display().to_string(),
                "size": size,
                "sha256": sha256,
            },
            "installFiles": [
                {
                    "identity": "mainExecutable",
                    "path": main.display().to_string(),
                    "size": main_size,
                    "sha256": main_sha256,
                },
                {
                    "identity": "maintenanceHelper",
                    "path": helper.display().to_string(),
                    "size": helper_size,
                    "sha256": helper_sha256,
                },
            ]
        });
    }
    println!("{report}");
    Ok(())
}
