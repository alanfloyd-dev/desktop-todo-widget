//! Operator CLI for the release packaging + publishing pipeline (Phase 3B).
//!
//! Subcommands:
//!
//! - `prepare`   — assemble → facts → manifest → sign → sidecar → reports
//! - `publish`   — provider upload (GitHub draft-first / Gitee) with the
//!                 explicit collision policy; `--dry-run` plans, `--upload`
//!                 executes, `--finalize` exposes a verified draft
//! - `read-back` — byte-exact remote verification + mirror consistency
//!
//! Publishing tools touch only public artifacts. Signing keys never appear
//! on any publish path; provider tokens come from environment variables and
//! are never logged. Rehearsal artifacts are refused on production
//! endpoints.

use std::path::PathBuf;
use std::process::ExitCode;

use desktop_todo_release_pipeline as pipeline;
use pipeline::publish::{ArtifactSet, Provider, PublishDecision};
use pipeline::report::VerificationRecord;
use pipeline::{ReleaseMode, PipelineError};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(error.exit_code() as u8)
        }
    }
}

struct Flags {
    values: std::collections::HashMap<String, String>,
    switches: std::collections::HashSet<String>,
}

fn parse_flags(
    args: &[String],
    values: &[&str],
    switches: &[&str],
) -> Result<Flags, PipelineError> {
    let mut parsed_values = std::collections::HashMap::new();
    let mut parsed_switches = std::collections::HashSet::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let Some(name) = arg.strip_prefix("--") else {
            return Err(PipelineError::Usage(format!(
                "unexpected positional argument {arg:?}"
            )));
        };
        if switches.contains(&name) {
            if !parsed_switches.insert(name.to_string()) {
                return Err(PipelineError::Usage(format!("flag --{name} given twice")));
            }
            index += 1;
            continue;
        }
        if !values.contains(&name) {
            return Err(PipelineError::Usage(format!("unknown flag --{name}")));
        }
        let Some(value) = args.get(index + 1) else {
            return Err(PipelineError::Usage(format!(
                "flag --{name} requires a value"
            )));
        };
        if value.starts_with("--") {
            return Err(PipelineError::Usage(format!(
                "flag --{name} requires a value"
            )));
        }
        if parsed_values.contains_key(name) {
            return Err(PipelineError::Usage(format!("flag --{name} given twice")));
        }
        parsed_values.insert(name.to_string(), value.clone());
        index += 2;
    }
    Ok(Flags {
        values: parsed_values,
        switches: parsed_switches,
    })
}

impl Flags {
    fn value(&self, name: &str) -> Result<String, PipelineError> {
        self.values.get(name).cloned().ok_or_else(|| {
            PipelineError::Usage(format!("missing required flag --{name}"))
        })
    }
    fn optional(&self, name: &str) -> Option<String> {
        self.values.get(name).cloned()
    }
    fn switch(&self, name: &str) -> bool {
        self.switches.contains(name)
    }
}

fn usage() -> Result<String, PipelineError> {
    let text = r#"release packaging + publishing pipeline for Maintenance Protocol 1 (Phase 3B)

USAGE:
  desktop-todo-release-pipeline prepare --staging <dir> --version <x.y.z> --published-at <rfc3339-utc>
        [--notes "<text>"] [--repo-root <dir>] [--main-exe <path>] [--helper-exe <path>]
        [--mode rehearsal --key <test-seed> --trust-public <test-public>]
        [--commit <sha>] [--overwrite] [--dry-run]
      (default mode is production: signing goes through the compiled production
       trust gate and fails closed while no key is provisioned)

  desktop-todo-release-pipeline publish (--github | --gitee) --staging <dir>
        [--repo <owner/name>] [--tag <vX.Y.Z>] [--api-base <url>] [--upload-base <url>]
        [--dry-run] [--upload] [--finalize]
      (--dry-run plans only; --upload executes with the collision policy;
       --finalize exposes a verified GitHub draft — requires a passed read-back)

  desktop-todo-release-pipeline read-back (--github | --gitee | --both) --staging <dir>
        [--repo <owner/name>] [--tag <vX.Y.Z>] [--api-base <url>] [--upload-base <url>]
        [--mode rehearsal --trust-public <test-public>]

Provider tokens come from DTW_GITHUB_TOKEN / DTW_GITEE_TOKEN (provider auth
only, never logged). Rehearsal artifacts touch production endpoints only as
GitHub DRAFT releases (finalize is refused in rehearsal mode); providers
without a draft state (Gitee) refuse rehearsal artifacts entirely.
See docs/release-signing.md and docs/application-lifecycle.md §16.
"#;
    Ok(text.to_string())
}

fn run(args: &[String]) -> Result<String, PipelineError> {
    let Some(command) = args.first() else {
        return usage();
    };
    let rest = &args[1..];
    match command.as_str() {
        "prepare" => prepare_command(rest),
        "publish" => publish_command(rest),
        "read-back" => read_back_command(rest),
        "--help" | "-h" | "help" => usage(),
        other => Err(PipelineError::Usage(format!(
            "unknown command {other:?}; run with --help"
        ))),
    }
}

/// Resolve the release mode from flags. Rehearsal requires the trust
/// material at use time; production must not receive any.
fn resolve_mode(flags: &Flags) -> Result<ReleaseMode, PipelineError> {
    match flags.optional("mode").as_deref() {
        Some("rehearsal") => Ok(ReleaseMode::Rehearsal),
        Some(other) => Err(PipelineError::Usage(format!(
            "unknown --mode {other:?}; only 'rehearsal' is explicit (default is production)"
        ))),
        None => Ok(ReleaseMode::Production),
    }
}

fn prepare_command(args: &[String]) -> Result<String, PipelineError> {
    let flags = parse_flags(
        args,
        &[
            "staging",
            "version",
            "published-at",
            "notes",
            "repo-root",
            "main-exe",
            "helper-exe",
            "mode",
            "key",
            "trust-public",
            "commit",
        ],
        &["overwrite", "dry-run"],
    )?;
    let mode = resolve_mode(&flags)?;
    let repo_root = flags
        .optional("repo-root")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("current directory"));
    // Fail closed on an unrecognizable repo root.
    if !repo_root.join("src-tauri/Cargo.toml").exists() {
        return Err(PipelineError::Usage(format!(
            "--repo-root {} does not look like the repository root (no src-tauri/Cargo.toml)",
            repo_root.display()
        )));
    }
    let options = pipeline::pipeline::PrepareOptions {
        staging: PathBuf::from(flags.value("staging")?),
        repo_root,
        main_exe: flags.optional("main-exe").map(PathBuf::from),
        helper_exe: flags.optional("helper-exe").map(PathBuf::from),
        version: flags.value("version")?,
        published_at: flags.value("published-at")?,
        notes: flags
            .optional("notes")
            .unwrap_or_else(|| "Application lifecycle management.".to_string()),
        mode,
        key_path: flags.optional("key").map(PathBuf::from),
        trust_public_path: flags.optional("trust-public").map(PathBuf::from),
        source_commit: flags.optional("commit"),
        overwrite: flags.switch("overwrite"),
    };
    pipeline::pipeline::prepare(&options, flags.switch("dry-run"))
}

/// Build the provider publisher for a command. Rehearsal mode requires an
/// explicit --api-base override (isolated endpoint); production defaults to
/// the compiled production endpoints and the production repository.
fn build_publisher(
    provider: Provider,
    flags: &Flags,
    mode: ReleaseMode,
) -> Result<Box<dyn pipeline::publish::ProviderApi>, PipelineError> {
    let api_base = flags.optional("api-base");
    let default_repo = flags
        .optional("repo")
        .unwrap_or_else(|| pipeline::publish::PRODUCTION_REPO.to_string());
    let validate_repo = |repo: &str| {
        let parts: Vec<&str> = repo.split('/').collect();
        if parts.len() != 2
            || parts.iter().any(|part| {
                part.is_empty() || part.contains("..") || part.contains(':') || part.contains('\\')
            })
        {
            Err(PipelineError::Usage(format!(
                "--repo {repo:?} must be <owner>/<name>"
            )))
        } else {
            Ok(())
        }
    };
    validate_repo(&default_repo)?;
    match provider {
        Provider::GitHub => {
            let upload_base = flags.optional("upload-base");
            match (api_base, upload_base) {
                (Some(api), Some(upload)) => {
                    if mode != ReleaseMode::Rehearsal {
                        return Err(PipelineError::RehearsalSafety {
                            detail: "--api-base/--upload-base overrides are rehearsal-only; \
                                     production publishes use the compiled endpoints"
                                .to_string(),
                        });
                    }
                    Ok(Box::new(pipeline::publish::GitHubPublisher::for_rehearsal(
                        &default_repo, api, upload,
                    )?))
                }
                (Some(_), None) => Err(PipelineError::Usage(
                    "--api-base for GitHub also requires --upload-base".to_string(),
                )),
                (None, _) => Ok(Box::new(pipeline::publish::GitHubPublisher::production(
                    &default_repo,
                )?)),
            }
        }
        Provider::Gitee => match api_base {
            Some(api) => {
                if mode != ReleaseMode::Rehearsal {
                    return Err(PipelineError::RehearsalSafety {
                        detail: "--api-base overrides are rehearsal-only; production publishes \
                                 use the compiled endpoints"
                            .to_string(),
                    });
                }
                Ok(Box::new(pipeline::publish::GiteePublisher::for_rehearsal(
                    &default_repo, api,
                )?))
            }
            None => Ok(Box::new(pipeline::publish::GiteePublisher::production(
                &default_repo,
            )?)),
        },
    }
}

fn publish_command(args: &[String]) -> Result<String, PipelineError> {
    let flags = parse_flags(
        args,
        &["staging", "repo", "tag", "api-base", "upload-base", "mode"],
        &["github", "gitee", "dry-run", "upload", "finalize"],
    )?;
    let provider = if flags.switch("github") ^ flags.switch("gitee") {
        if flags.switch("github") {
            Provider::GitHub
        } else {
            Provider::Gitee
        }
    } else {
        return Err(PipelineError::Usage(
            "pass exactly one of --github / --gitee".to_string(),
        ));
    };
    let mode = resolve_mode(&flags)?;
    let staging = PathBuf::from(flags.value("staging")?);
    let facts = load_facts(&staging)?;
    if facts.mode != mode {
        return Err(PipelineError::ModeConflict {
            detail: format!(
                "staging artifacts are {:?} but the command carries {:?} flags",
                facts.mode, mode
            ),
        });
    }
    let artifacts = ArtifactSet::from_staging(&staging, &facts);
    let tag = match flags.optional("tag") {
        Some(tag) => tag,
        None => pipeline::publish::tag_for_version(&facts.target_version)?,
    };
    let api = build_publisher(provider, &flags, mode)?;

    if flags.switch("finalize") {
        // Exposure gate: the release may only leave draft state after its
        // read-back verification passed (§16.6) — and a rehearsal release
        // must never be exposed through a production endpoint at all.
        pipeline::publish::enforce_finalize_safety(api.as_ref(), mode)?;
        if !pipeline::report::provider_verified(&staging, provider.as_str(), &tag)? {
            return Err(PipelineError::ReadBack {
                detail: format!(
                    "finalize refused: no passed read-back verification for {provider_name} {tag} \
                     in this staging root (run `read-back` first)",
                    provider_name = provider.as_str()
                ),
            });
        }
        let release = api
            .find_release(&tag)?
            .ok_or(PipelineError::ReadBack {
                detail: format!("no remote release exists for {tag}"),
            })?;
        api.set_publication(&release, false)?;
        return Ok(format!(
            "finalized {tag} on {}: the release is now visible to updater discovery",
            provider.as_str()
        ));
    }

    if flags.switch("dry-run") {
        let (_, decisions) =
            pipeline::publish::plan_publish(api.as_ref(), &artifacts, &tag, mode)?;
        let mut text = format!(
            "publish dry run for {tag} on {} (no mutation performed)\n",
            provider.as_str()
        );
        for decision in &decisions {
            match decision {
                PublishDecision::CreateRelease { draft } => text.push_str(&format!(
                    "  create release {tag} (draft: {draft})\n"
                )),
                PublishDecision::UploadAsset { name } => {
                    text.push_str(&format!("  upload asset {name}\n"))
                }
                PublishDecision::IdempotentAsset { name } => {
                    text.push_str(&format!("  asset {name} already present with identical bytes: idempotent\n"))
                }
            }
        }
        if provider == Provider::Gitee {
            text.push_str(
                "  note: Gitee has no draft state — an executed publish is immediately visible\n",
            );
        }
        return Ok(text);
    }

    if flags.switch("upload") {
        // The release title/body must be unambiguous about the mode: a
        // rehearsal release is visibly marked non-production on the
        // provider, and carries no local paths, tokens, or key material.
        let (title, body) = match mode {
            ReleaseMode::Rehearsal => (
                format!(
                    "REHEARSAL (DO NOT USE) desktop-todo-widget {}",
                    facts.target_version
                ),
                "NON-PRODUCTION updater smoke rehearsal. This release is signed with a TEST \
                 key that verifies under NO production trust store; it must never be \
                 finalized or installed. Safe to delete."
                    .to_string(),
            ),
            ReleaseMode::Production => (
                format!("desktop-todo-widget {}", facts.target_version),
                "Application lifecycle management.".to_string(),
            ),
        };
        let (release, decisions) =
            pipeline::publish::execute_publish(api.as_ref(), &artifacts, &tag, &title, &body, mode)?;
        let mut text = format!("published {tag} on {}:\n", provider.as_str());
        for decision in &decisions {
            match decision {
                PublishDecision::CreateRelease { draft } => text.push_str(&format!(
                    "  created release {tag} (draft: {draft})\n"
                )),
                PublishDecision::UploadAsset { name } => {
                    text.push_str(&format!("  uploaded asset {name}\n"))
                }
                PublishDecision::IdempotentAsset { name } => {
                    text.push_str(&format!("  asset {name} already present with identical bytes\n"))
                }
            }
        }
        let _ = release;
        if provider == Provider::GitHub {
            text.push_str(
                "  release is a DRAFT: run `read-back --github`, then `publish --github --finalize`\n",
            );
        } else {
            text.push_str(
                "  note: Gitee has no draft state — this release is already visible\n",
            );
        }
        return Ok(text);
    }

    Err(PipelineError::Usage(
        "publish is read-only by default: pass --dry-run to plan or --upload to execute \
         (mutating the provider is never implicit)"
            .to_string(),
    ))
}

fn read_back_command(args: &[String]) -> Result<String, PipelineError> {
    let flags = parse_flags(
        args,
        &[
            "staging",
            "repo",
            "tag",
            "api-base",
            "upload-base",
            "mode",
            "trust-public",
        ],
        &["github", "gitee", "both"],
    )?;
    let providers: Vec<Provider> = if flags.switch("both") {
        if flags.switch("github") || flags.switch("gitee") {
            return Err(PipelineError::Usage(
                "--both excludes --github/--gitee".to_string(),
            ));
        }
        vec![Provider::GitHub, Provider::Gitee]
    } else if flags.switch("github") && !flags.switch("gitee") {
        vec![Provider::GitHub]
    } else if flags.switch("gitee") && !flags.switch("github") {
        vec![Provider::Gitee]
    } else {
        return Err(PipelineError::Usage(
            "pass --github, --gitee, or --both".to_string(),
        ));
    };
    let mode = resolve_mode(&flags)?;
    let staging = PathBuf::from(flags.value("staging")?);
    let facts = load_facts(&staging)?;
    if facts.mode != mode {
        return Err(PipelineError::ModeConflict {
            detail: format!(
                "staging artifacts are {:?} but the command carries {:?} flags",
                facts.mode, mode
            ),
        });
    }
    let artifacts = ArtifactSet::from_staging(&staging, &facts);
    let tag = match flags.optional("tag") {
        Some(tag) => tag,
        None => pipeline::publish::tag_for_version(&facts.target_version)?,
    };
    let trust = pipeline::signing::verification_trust(mode, flags.optional("trust-public").map(PathBuf::from).as_deref())?;

    let mut records: Vec<VerificationRecord> = Vec::new();
    let mut results = Vec::new();
    for provider in providers {
        let api = build_publisher(provider, &flags, mode)?;
        match pipeline::publish::read_back(api.as_ref(), &artifacts, &facts, &trust, &staging, &tag)
        {
            Ok(result) => {
                records.push(VerificationRecord::from_result(&result, &tag, mode));
                results.push(result);
            }
            Err(error) => {
                records.push(VerificationRecord::failure(
                    provider.as_str(),
                    &tag,
                    mode,
                    error.to_string(),
                ));
                pipeline::report::write_verification_report(&staging, &records)?;
                return Err(error);
            }
        }
    }
    // Mirror consistency across every verified provider pair (§14).
    if results.len() == 2 {
        pipeline::publish::assert_mirror_consistency(&results[0], &results[1])?;
    }
    pipeline::report::write_verification_report(&staging, &records)?;
    let mut text = format!("read-back verification passed for {tag}:\n");
    for result in &results {
        text.push_str(&format!(
            "  {}: {} artifacts verified, manifest sha256 {}\n",
            result.provider,
            result.artifact_sha256.len(),
            result.manifest_sha256_hex
        ));
    }
    if results.len() == 2 {
        text.push_str("  mirror consistency: providers agree byte-for-byte\n");
    }
    Ok(text)
}

fn load_facts(
    staging: &std::path::Path,
) -> Result<pipeline::facts::ReleaseFacts, PipelineError> {
    pipeline::report::read_facts(staging)
}
