//! The manual update check (the first slice of the in-app updater surface):
//! thin commands over the existing discovery/acquisition engine.
//!
//! Trust posture: this module adds no trust path. `check_for_updates` runs
//! the frozen `discover` pipeline (compiled trust store, compiled endpoints,
//! bounded scan) and maps the outcome to a UI status — it writes no durable
//! state, exactly like every other discovery caller. `download_update` runs
//! the same bounded discovery once more and then drives only the existing
//! durable milestones (`persist_trusted_target` → `acquire_and_stage`); the
//! staged session is installed exclusively through the frozen
//! `install_ready_update(sessionId)` command, whose contract is untouched.
//! The frontend supplies no path, URL, digest, version, or session fact —
//! the same single-input rule as `install_ready_update`.
//!
//! Running discovery twice (check, then download) is deliberate: discovery
//! is bounded and side-effect-free, and the newest provider state may have
//! moved between the two user actions. The staged session always names the
//! target discovery accepted at download time, and `install_ready_update`
//! re-authenticates it from persisted bytes.
//!
//! Hop policy: [`CompiledHops`] is the publisher-reviewed rollback
//! compatibility fact the protocol requires the client to compile ("a direct
//! hop is eligible only when every durable write the target runtime can
//! perform before HealthAck is proven compatible with a rollback runtime at
//! `source`"). Only patch hops inside a reviewed minor series are admitted
//! today (v1.2.x → v1.2.y); no cross-minor direct hop has been reviewed, so
//! a future v1.3.0 stays invisible to this client until the publisher
//! extends the policy — the frozen "highest reachable safe next hop"
//! behavior, not an availability failure.

use serde::Serialize;

use desktop_todo_update_core::{RollbackCompatibility, TrustStore, Version};

use crate::maintenance_admission::StartupContext;
use crate::updater::acquisition::{
    acquire_and_stage, persist_trusted_target, AcquisitionError, PersistError,
};
use crate::updater::http_fetch::{discovery_client, FetchError};
use crate::updater::providers::{
    combined_origin_allowlist, production_endpoints, ProviderEndpoints, ProviderKind,
};
use crate::updater::{
    discover, CandidateResultKind, DiscoveryConfig, DiscoveryContext, DiscoveryOutcome,
    ReleaseSource, ScanReport,
};

/// The compiled rollback-compatibility policy: a direct hop is eligible only
/// within the same major.minor series, where every pre-HealthAck durable
/// write is the same reviewed schema the installed runtime already handles.
/// Cross-minor targets are ineligible until the publisher reviews and
/// extends this policy alongside that release.
struct CompiledHops;

impl RollbackCompatibility for CompiledHops {
    fn hop_eligible(&self, source: &Version, target: &Version) -> bool {
        let source = source.components();
        let target = target.components();
        source[0] == target[0] && source[1] == target[1]
    }
}

/// The check result the frontend renders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum UpdateCheckStatus {
    UpToDate,
    UpdateAvailable { version: String },
}

/// A staged, install-ready update session. `session_id` is the only value
/// the frontend hands back — to the frozen `install_ready_update` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StagedUpdate {
    pub session_id: String,
    pub version: String,
}

/// Why a manual check or download could not complete. The `Display` strings
/// are the frontend's pinned vocabulary, the same contract as `InstallError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    /// The canonical installation could not be resolved into a proven
    /// installed source version (no receipt, not installed, dev copy).
    InstallationUnresolved,
    /// The discovery layer stayed unavailable after bounded retries. A
    /// dead network must never masquerade as "no update available".
    DiscoveryUnavailable { detail: String },
    /// The bounded scan completed and every candidate was ineligible or
    /// invalid — nothing new this client can take.
    NoEligibleCandidate,
    /// The durable prepare (persist or stage) failed after the target was
    /// accepted. The session may be resumable; the app stays alive.
    Download(String),
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InstallationUnresolved => write!(f, "installation-unresolved"),
            Self::DiscoveryUnavailable { detail } => {
                write!(f, "discovery-unavailable: {detail}")
            }
            Self::NoEligibleCandidate => write!(f, "no-eligible-candidate"),
            Self::Download(detail) => write!(f, "download-failed: {detail}"),
        }
    }
}

impl From<PersistError> for CheckError {
    fn from(error: PersistError) -> Self {
        Self::Download(error.to_string())
    }
}

impl From<AcquisitionError> for CheckError {
    fn from(error: AcquisitionError) -> Self {
        Self::Download(error.to_string())
    }
}

/// Provider name in the frozen `discoveredVia` envelope vocabulary.
fn provider_name(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::GitHub => "GitHub",
        ProviderKind::Gitee => "Gitee",
    }
}

/// The provider whose bounded scan accepted the target.
fn accepted_provider(scans: &[ScanReport]) -> &'static str {
    for scan in scans {
        if scan
            .records
            .iter()
            .any(|record| matches!(record.result, CandidateResultKind::Accepted))
        {
            return provider_name(scan.provider);
        }
    }
    "unknown"
}

/// Interactive manual-check transport policy: the frozen contract pins no
/// durations, so this knob set is implementation policy. Under the
/// `discovery-qa` feature the QA seam may widen candidate visibility
/// (prereleases) exactly as for the pipeline probes; production builds
/// never read any environment variable here.
fn check_config() -> DiscoveryConfig {
    #[cfg(feature = "discovery-qa")]
    {
        crate::updater::providers::qa_config_from_env(DiscoveryConfig::default())
    }
    #[cfg(not(feature = "discovery-qa"))]
    {
        DiscoveryConfig::default()
    }
}

/// Run one bounded Auto discovery. The caller owns config/endpoints/trust so
/// the deterministic tests inject the mock endpoints; the built client is
/// returned because package acquisition rides the same allowlisted policy.
fn run_discovery(
    config: &DiscoveryConfig,
    endpoints: &[ProviderEndpoints],
    trust: &TrustStore,
    installed: &Version,
    hop: &dyn RollbackCompatibility,
) -> Result<(reqwest::blocking::Client, DiscoveryOutcome), CheckError> {
    let client = discovery_client(config, &combined_origin_allowlist(endpoints)).map_err(|e| {
        CheckError::DiscoveryUnavailable {
            detail: e.to_string(),
        }
    })?;
    let ctx = DiscoveryContext {
        config,
        scan_started: std::time::Instant::now(),
        client: &client,
        endpoints,
        trust,
        installed_version: installed,
        hop_policy: hop,
    };
    let outcome = discover(ReleaseSource::Auto, &ctx);
    Ok((client, outcome))
}

/// Classify one discovery outcome for the UI. A target's version comes from
/// its verified manifest — never from a provider tag.
fn map_check_outcome(outcome: DiscoveryOutcome) -> Result<UpdateCheckStatus, CheckError> {
    match outcome {
        DiscoveryOutcome::TargetFound { target, .. } => Ok(UpdateCheckStatus::UpdateAvailable {
            version: target.manifest().version().to_string(),
        }),
        DiscoveryOutcome::NoEligibleCandidate { .. } => Ok(UpdateCheckStatus::UpToDate),
        DiscoveryOutcome::DiscoveryUnavailable { provider, error, .. } => {
            Err(CheckError::DiscoveryUnavailable {
                detail: format!("{}: {error}", provider_name(provider)),
            })
        }
    }
}

/// The check core: discover, map, never persist.
pub(crate) fn check_with(
    config: &DiscoveryConfig,
    endpoints: &[ProviderEndpoints],
    trust: &TrustStore,
    installed: &Version,
    hop: &dyn RollbackCompatibility,
) -> Result<UpdateCheckStatus, CheckError> {
    let (_, outcome) = run_discovery(config, endpoints, trust, installed, hop)?;
    map_check_outcome(outcome)
}

/// The download core: discover once more, persist the trusted-target
/// milestone, download and stage the package. Returns the session id for
/// the frozen install command — nothing else crosses to the frontend.
pub(crate) fn download_with(
    updates_root: &std::path::Path,
    config: &DiscoveryConfig,
    endpoints: &[ProviderEndpoints],
    trust: &TrustStore,
    installed: &Version,
    hop: &dyn RollbackCompatibility,
) -> Result<StagedUpdate, CheckError> {
    let (client, outcome) = run_discovery(config, endpoints, trust, installed, hop)?;
    let (target, envelope_bytes, package_url, discovered_via) = match outcome {
        DiscoveryOutcome::TargetFound {
            target,
            envelope_bytes,
            package_url,
            scans,
        } => (*target, envelope_bytes, package_url, accepted_provider(&scans)),
        DiscoveryOutcome::NoEligibleCandidate { .. } => return Err(CheckError::NoEligibleCandidate),
        DiscoveryOutcome::DiscoveryUnavailable { provider, error, .. } => {
            return Err(CheckError::DiscoveryUnavailable {
                detail: format!("{}: {error}", provider_name(provider)),
            });
        }
    };
    let mut session = persist_trusted_target(
        updates_root,
        target,
        &envelope_bytes,
        package_url.as_deref(),
        discovered_via,
        installed,
    )?;
    acquire_and_stage(&mut session, &client)?;
    Ok(StagedUpdate {
        session_id: session.id,
        version: session.record.target_version,
    })
}

/// The production inputs both commands share: the proven installed source
/// version, the compiled trust store, the compiled endpoints, and the
/// interactive check config.
struct ProductionInputs {
    installed: Version,
    trust: TrustStore,
    endpoints: Vec<ProviderEndpoints>,
    config: DiscoveryConfig,
}

fn production_inputs() -> Result<ProductionInputs, CheckError> {
    let paths = desktop_todo_maintenance::paths::Paths::resolve()
        .map_err(|_| CheckError::InstallationUnresolved)?;
    let installed = crate::updater::spawn::confirm_installed_source(&paths)
        .map_err(|_| CheckError::InstallationUnresolved)?;
    Ok(ProductionInputs {
        installed,
        trust: desktop_todo_update_core::production_trust_store(),
        endpoints: production_endpoints(),
        config: check_config(),
    })
}

/// The frontend command: `check_for_updates()`. Read-only — discovery never
/// persists state.
#[tauri::command]
pub async fn check_for_updates() -> Result<UpdateCheckStatus, String> {
    tauri::async_runtime::spawn_blocking(|| -> Result<UpdateCheckStatus, CheckError> {
        let inputs = production_inputs()?;
        check_with(
            &inputs.config,
            &inputs.endpoints,
            &inputs.trust,
            &inputs.installed,
            &CompiledHops,
        )
    })
    .await
    .map_err(|e| format!("check-failed: {e}"))?
    .map_err(|error| error.to_string())
}

/// The frontend command: `download_update()`. Managed installations only —
/// it creates durable state and hands back a session the frozen install
/// command can drive. Blocking work runs off the main thread.
#[tauri::command]
pub async fn download_update(
    admission: tauri::State<'_, crate::maintenance_admission::Admission>,
) -> Result<StagedUpdate, String> {
    if admission.context != StartupContext::Managed {
        return Err(crate::updater::install::InstallError::NotManaged.to_string());
    }
    tauri::async_runtime::spawn_blocking(|| -> Result<StagedUpdate, CheckError> {
        let paths = desktop_todo_maintenance::paths::Paths::resolve()
            .map_err(|_| CheckError::InstallationUnresolved)?;
        let installed = crate::updater::spawn::confirm_installed_source(&paths)
            .map_err(|_| CheckError::InstallationUnresolved)?;
        download_with(
            &paths.state().join("updates"),
            &check_config(),
            &production_endpoints(),
            &desktop_todo_update_core::production_trust_store(),
            &installed,
            &CompiledHops,
        )
    })
    .await
    .map_err(|e| format!("download-failed: {e}"))?
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::updater::CandidateRecord;

    /// The closed refusal taxonomy stays stable — these strings are the
    /// whole surface the frontend maps to localized copy.
    #[test]
    fn error_taxonomy_is_stable() {
        assert_eq!(
            CheckError::InstallationUnresolved.to_string(),
            "installation-unresolved"
        );
        assert_eq!(CheckError::NoEligibleCandidate.to_string(), "no-eligible-candidate");
        assert_eq!(
            CheckError::DiscoveryUnavailable {
                detail: "GitHub: transport failure: unreachable".to_string(),
            }
            .to_string(),
            "discovery-unavailable: GitHub: transport failure: unreachable"
        );
        assert_eq!(
            CheckError::Download("persist refused".to_string()).to_string(),
            "download-failed: persist refused"
        );
    }

    #[test]
    fn compiled_hops_admit_only_patch_series() {
        let hops = CompiledHops;
        let v = |text: &str| Version::parse(text).unwrap();
        assert!(hops.hop_eligible(&v("1.2.0"), &v("1.2.1")));
        assert!(hops.hop_eligible(&v("1.2.1"), &v("1.2.9")));
        assert!(!hops.hop_eligible(&v("1.2.0"), &v("1.3.0")));
        assert!(!hops.hop_eligible(&v("1.2.0"), &v("2.0.0")));
        assert!(!hops.hop_eligible(&v("1.3.0"), &v("1.2.9")));
    }

    /// No compiled endpoints is a discovery-layer failure, never an "up to
    /// date" answer — a dead layer must stay distinguishable from a
    /// completed scan, and this path exercises it without any network.
    #[test]
    fn missing_endpoints_fail_as_discovery_unavailable() {
        let status = check_with(
            &DiscoveryConfig::default(),
            &[],
            &TrustStore::from_raw_keys(&[[0u8; 32]]).unwrap(),
            &Version::parse("1.2.0").unwrap(),
            &CompiledHops,
        );
        match status {
            Err(CheckError::DiscoveryUnavailable { .. }) => {}
            other => panic!("expected DiscoveryUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn discovery_unavailable_maps_to_stable_error() {
        let outcome = DiscoveryOutcome::DiscoveryUnavailable {
            provider: ProviderKind::Gitee,
            error: FetchError::Timeout {
                detail: "bounded timeout".to_string(),
            },
            scans: vec![ScanReport {
                provider: ProviderKind::Gitee,
                records: Vec::new(),
                skipped_draft: 0,
                skipped_prerelease: 0,
                budget_exhausted: false,
            }],
        };
        match map_check_outcome(outcome) {
            Err(error) => {
                assert!(error.to_string().starts_with("discovery-unavailable: Gitee"))
            }
            Ok(status) => panic!("expected error, got {status:?}"),
        }
    }

    #[test]
    fn accepted_provider_uses_envelope_vocabulary() {
        let accepted = |kind| ScanReport {
            provider: kind,
            records: vec![CandidateRecord {
                kind,
                release_id: "1".to_string(),
                tag: "v1.2.1".to_string(),
                result: CandidateResultKind::Accepted,
            }],
            skipped_draft: 0,
            skipped_prerelease: 0,
            budget_exhausted: false,
        };
        assert_eq!(accepted_provider(&[accepted(ProviderKind::GitHub)]), "GitHub");
        assert_eq!(accepted_provider(&[accepted(ProviderKind::Gitee)]), "Gitee");
        assert_eq!(accepted_provider(&[]), "unknown");
    }

    /// The interactive config stays bounded and prerelease-filtered in
    /// production builds; under `discovery-qa` only the visibility seam may
    /// flip.
    #[test]
    fn config_defaults_are_bounded() {
        let config = check_config();
        assert!(config.scan_budget.is_some());
        assert_eq!(config.max_retries, 2);
    }
}
