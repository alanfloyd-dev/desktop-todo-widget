//! Updater provider discovery (Phase 2B-B): GitHub / Gitee / Auto bounded
//! candidate enumeration plus bounded metadata fetch, feeding exact raw
//! bytes into the shared `desktop-todo-update-core` verifier.
//!
//! Trust boundary (frozen): the providers sit entirely outside the trust
//! boundary. They answer only "where is a candidate release?"; everything
//! they return is attacker-controlled; provider identity never grants trust;
//! GitHub is never "more trusted" than Gitee; and Auto's fallback never
//! changes cryptographic policy. A candidate becomes a target only through
//! `update_core::evaluate_candidate` — the compiled trust store plus the
//! shared verifier, the same path the maintenance helper uses.
//!
//! This phase writes no durable state: an accepted target lives in memory
//! only, for later phases to persist and act on.

pub(crate) mod acquisition;
pub(crate) mod http_fetch;
pub(crate) mod package_zip;
pub(crate) mod providers;
#[cfg(test)]
mod tests;

use std::time::{Duration, Instant};

use desktop_todo_update_core::{
    evaluate_candidate, CandidateInput, ErrorKind, IneligibilityReason, RollbackCompatibility,
    SelectionContext, TrustStore, VerifiedTarget, Version,
};

use crate::updater::http_fetch::{fetch_bounded, BodyKind, FetchError};
use crate::updater::providers::{enumerate_candidates, ProviderEndpoints, ProviderKind};

/// Which discovery source a check runs against. A transport preference only:
/// all sources meet the identical signature trust.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseSource {
    Auto,
    GitHub,
    Gitee,
}

/// Transport-level discovery policy. **Every value here is implementation
/// transport policy, NOT a signed protocol semantic rule.** The frozen
/// contract fixes only the envelope/manifest byte limits (enforced inside
/// update-core) and requires bounded enumeration, bounded response sizes,
/// and finite timeouts; it pins no durations, page counts, or retry counts
/// beyond the frozen retry shape (initial attempt plus two retries,
/// respecting bounded Retry-After).
#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    /// The frozen `listCandidates(channel, boundedLimit)` bound: the scan
    /// depth handed to the provider as its page size and never exceeded.
    pub max_candidates: usize,
    /// Attacker-controlled provider index response cap, applied before
    /// parsing.
    pub provider_index_body_cap: usize,
    pub connect_timeout: Duration,
    /// Overall per-request timeout, including the bounded body read.
    pub request_timeout: Duration,
    /// Frozen retry shape: one initial attempt plus this many transient
    /// retries.
    pub max_retries: u32,
    /// Fixed backoff between retries when no Retry-After is offered.
    pub retry_backoff: Duration,
    /// Upper bound for any single Retry-After sleep ("bounded Retry-After").
    pub retry_after_cap: Duration,
    /// Redirect attempts the allowlisted policy may follow.
    pub max_redirects: usize,
    /// Whole-scan budget (implementation transport policy, never a signed
    /// protocol semantic rule): once elapsed, the scan stops taking new
    /// candidates instead of running per-request timeouts × retries ×
    /// candidates unbounded. Expiry is never a candidate verdict, never
    /// discards an accepted target, and is not a provider-layer failure, so
    /// the frozen Auto fallback does not react to it.
    pub scan_budget: Option<Duration>,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            max_candidates: 10,
            provider_index_body_cap: 1024 * 1024,
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
            max_retries: 2,
            retry_backoff: Duration::from_millis(250),
            retry_after_cap: Duration::from_secs(5),
            max_redirects: 4,
            scan_budget: Some(Duration::from_secs(120)),
        }
    }
}

/// Run one provider call behind the bounded retry ladder: initial attempt
/// plus `max_retries` transient retries, sleeping at most `retry_after_cap`
/// per Retry-After and `retry_backoff` otherwise.
/// Deadline derived from the whole-scan budget: `None` means unlimited.
pub(crate) fn scan_deadline(ctx: &DiscoveryContext<'_>) -> Option<Instant> {
    ctx.config
        .scan_budget
        .and_then(|budget| ctx.scan_started.checked_add(budget))
}

/// Remaining whole-scan budget right now, if budgeted.
pub(crate) fn remaining_budget(deadline: Option<Instant>) -> Option<Duration> {
    deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()))
}

/// Run one provider call behind the bounded retry ladder with a hard
/// whole-scan deadline. Every retry attempt's request timeout is
/// `min(configured timeout, remaining budget)`; a Retry-After or backoff
/// sleep longer than the remaining budget ends the scan immediately instead
/// of sleeping past the deadline. Budget expiry is a client-side stop — it
/// is never retried, never a candidate verdict, and never a provider-layer
/// failure.
pub(crate) fn with_bounded_retry<T>(
    config: &DiscoveryConfig,
    deadline: Option<Instant>,
    call: impl Fn(Duration) -> Result<T, FetchError>,
) -> Result<T, FetchError> {
    let attempts = config.max_retries.saturating_add(1).max(1);
    for attempt in 0..attempts {
        // Budget gate before every attempt: no request may start past the
        // deadline. `None` deadline means unlimited.
        let per_request_timeout = match remaining_budget(deadline) {
            Some(remaining) if remaining.is_zero() => {
                return Err(FetchError::BudgetExhausted);
            }
            Some(remaining) => remaining.min(config.request_timeout),
            None => config.request_timeout,
        };
        match call(per_request_timeout) {
            Err(error) if error.is_transient() && attempt + 1 < attempts => {
                let wanted = match &error {
                    FetchError::HttpStatus {
                        retry_after: Some(delay),
                        ..
                    } => (*delay).min(config.retry_after_cap),
                    _ => config.retry_backoff,
                };
                // A sleep longer than the remaining budget is not taken:
                // the scan stops instead of hanging past the deadline.
                match remaining_budget(deadline) {
                    Some(remaining) if wanted < remaining && !remaining.is_zero() => {
                        if !wanted.is_zero() {
                            std::thread::sleep(wanted);
                        }
                    }
                    Some(_) => return Err(FetchError::BudgetExhausted),
                    None => {
                        if !wanted.is_zero() {
                            std::thread::sleep(wanted);
                        }
                    }
                }
            }
            other => return other,
        }
    }
    unreachable!("retry loop returns inside the match")
}

/// Why a scanned candidate did not become the target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateResultKind {
    Accepted,
    /// Malformed or unverifiable candidate (update-core classification).
    Invalid(ErrorKind),
    /// A genuine candidate this client cannot or must not take (update-core
    /// classification).
    Ineligible(IneligibilityReason),
    /// Candidate-local unavailability of the metadata assets — the provider
    /// published garbage (missing assets, off-allowlist URL, oversized body,
    /// or a redirect outside the allowlist). The bounded scan continues past
    /// it, so it can never block an older bridge.
    MetadataUnavailable {
        detail: String,
    },
}

/// Per-candidate scan record (diagnostics only).
#[derive(Debug, Clone)]
pub struct CandidateRecord {
    pub kind: ProviderKind,
    pub release_id: String,
    pub tag: String,
    pub result: CandidateResultKind,
}

/// Diagnostics for one provider's bounded scan.
#[derive(Debug, Clone)]
pub struct ScanReport {
    pub provider: ProviderKind,
    pub records: Vec<CandidateRecord>,
    pub skipped_draft: usize,
    pub skipped_prerelease: usize,
    /// True when the whole-scan budget expired before every candidate could
    /// be taken. A client-side stop — recorded, never a candidate verdict,
    /// never a provider-layer failure.
    pub budget_exhausted: bool,
}

/// The result of a discovery run. A transport failure is a different layer
/// from candidate verdicts: it never classifies a candidate and never
/// reaches the verifier.
#[derive(Debug)]
pub enum DiscoveryOutcome {
    /// The first candidate whose complete eligibility predicate succeeded.
    /// In memory only this phase — no durable state exists yet. The package
    /// locator comes from the accepted candidate's provider assets and is
    /// transport metadata only.
    TargetFound {
        target: Box<VerifiedTarget>,
        /// Exact raw envelope bytes of the accepted candidate.
        envelope_bytes: Vec<u8>,
        package_url: Option<String>,
        scans: Vec<ScanReport>,
    },
    /// Every bounded scan completed without an eligible candidate.
    NoEligibleCandidate { scans: Vec<ScanReport> },
    /// One provider's discovery layer stayed unavailable after bounded
    /// retries (transport failure, timeout, malformed provider response, or
    /// an unusable releases endpoint). Never mixed with candidate verdicts.
    DiscoveryUnavailable {
        provider: ProviderKind,
        error: FetchError,
        scans: Vec<ScanReport>,
    },
}

/// Everything one discovery run needs. The trust store, installed version,
/// and hop policy are the caller's compiled/validated inputs; the pipeline
/// owns no trust state of its own.
pub struct DiscoveryContext<'a> {
    pub config: &'a DiscoveryConfig,
    /// Start of the current discovery run; the whole-scan budget is measured
    /// from here so retries and candidates share one finite budget.
    pub scan_started: Instant,
    pub client: &'a reqwest::blocking::Client,
    pub endpoints: &'a [ProviderEndpoints],
    pub trust: &'a TrustStore,
    /// The proven-consistent installed version anchor (installed-version
    /// authority). Candidate selection never runs against an inconsistent
    /// evidence set.
    pub installed_version: &'a Version,
    pub hop_policy: &'a dyn RollbackCompatibility,
}

fn endpoints_for<'a>(
    ctx: &'a DiscoveryContext<'a>,
    kind: ProviderKind,
) -> Result<&'a ProviderEndpoints, ProviderKind> {
    ctx.endpoints
        .iter()
        .find(|endpoint| endpoint.kind == kind)
        .ok_or(kind)
}

/// One provider's bounded scan: enumerate newest→oldest, fetch the bounded
/// metadata bytes per candidate, and run the shared verifier/eligibility
/// path on each. Stops at the first acceptance.
///
/// Transport-failure semantics, derived from the frozen contract rather
/// than convenience: the frozen scan continues past candidate-invalid and
/// candidate-ineligible results so the provider gains no lever to block a
/// still-valid bridge, and the frozen answer to a hidden bridge is "an
/// availability failure, not a trust bypass" (protocol v1 final Q15) — so a
/// candidate whose metadata fetch fails at transport level (timeouts,
/// connection resets, unexpected statuses, body-read errors — after the
/// bounded retry ladder) also rejects only itself and the scan continues.
/// "Discovery layer unavailable as a whole" — the only state Auto's frozen
/// fallback reacts to — means the provider served **no evaluation at all**:
/// either the enumeration failed, or every enumerated candidate's metadata
/// fetch failed at transport level (zero candidates evaluated). A scan
/// where at least one candidate was evaluated is **partial degradation**
/// and completes normally (no-target or target), with failures recorded.
struct ScanResult {
    target: Option<VerifiedTarget>,
    /// Exact raw envelope bytes of the accepted candidate, for durable
    /// trusted-target persistence (the manifest bytes live in the target).
    envelope_bytes: Vec<u8>,
    /// Package ZIP locator from the accepted candidate's provider assets.
    /// Transport metadata only — the signed package filename/size/SHA-256
    /// are the identity, and acquisition re-validates the URL against the
    /// signed filename and the origin allowlist.
    package_url: Option<String>,
    report: ScanReport,
}

fn scan_provider(
    ctx: &DiscoveryContext<'_>,
    endpoints: &ProviderEndpoints,
) -> Result<ScanResult, (FetchError, ScanReport)> {
    let mut report = ScanReport {
        provider: endpoints.kind,
        records: Vec::new(),
        skipped_draft: 0,
        skipped_prerelease: 0,
        budget_exhausted: false,
    };
    // The whole-scan deadline starts when the provider scan starts. `None`
    // means no budget (used by tests); a budget of zero expires immediately.
    let deadline = scan_deadline(ctx);

    let enumeration = match enumerate_candidates(ctx.client, endpoints, ctx.config, deadline) {
        Err(error) if error.is_budget_exhausted() => {
            // Client-side budget stop: the scan completes with nothing
            // taken, and the frozen Auto fallback does not react (this is
            // not a provider-layer failure).
            report.budget_exhausted = true;
            return Ok(ScanResult {
                target: None,
                envelope_bytes: Vec::new(),
                package_url: None,
                report,
            });
        }
        Ok(enumeration) => enumeration,
        Err(error) => return Err((error, report)),
    };
    report.skipped_draft = enumeration.skipped_draft;
    report.skipped_prerelease = enumeration.skipped_prerelease;

    let selection = SelectionContext {
        trust: ctx.trust,
        installed_version: ctx.installed_version,
        hop_policy: ctx.hop_policy,
    };

    // Transport-failure semantics (frozen-derived, see scan_provider doc):
    // a candidate whose metadata fetch fails at transport level rejects only
    // itself; the scan declares the whole layer unavailable only when not a
    // single candidate could be evaluated at all.
    let mut evaluated = 0usize;
    let mut transport_failures = 0usize;
    let mut last_transport_error: Option<FetchError> = None;

    for descriptor in enumeration.candidates {
        // Whole-scan budget: stop taking new candidates once expired. Not a
        // candidate verdict and not a provider failure — the scan simply
        // completes with what it has.
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            report.budget_exhausted = true;
            break;
        }
        // Candidate-local metadata unavailability: garbage rejects only
        // itself; the bounded scan continues to older candidates.
        let Ok(urls) = descriptor.metadata else {
            report.records.push(CandidateRecord {
                kind: descriptor.kind,
                release_id: descriptor.release_id.clone(),
                tag: descriptor.tag.clone(),
                result: CandidateResultKind::MetadataUnavailable {
                    detail: "metadata assets missing or off-allowlist".to_string(),
                },
            });
            continue;
        };
        let (envelope, manifest) = match fetch_metadata_pair(ctx, &urls, deadline) {
            Ok(pair) => pair,
            Err(MetadataFetchError::CandidateLocal(reason)) => {
                report.records.push(CandidateRecord {
                    kind: descriptor.kind,
                    release_id: descriptor.release_id.clone(),
                    tag: descriptor.tag.clone(),
                    result: reason,
                });
                continue;
            }
            Err(MetadataFetchError::Transport(error)) => {
                transport_failures += 1;
                last_transport_error = Some(error.clone());
                report.records.push(CandidateRecord {
                    kind: descriptor.kind,
                    release_id: descriptor.release_id.clone(),
                    tag: descriptor.tag.clone(),
                    result: CandidateResultKind::MetadataUnavailable {
                        detail: format!("metadata fetch failed at transport level: {error}"),
                    },
                });
                continue;
            }
        };

        // The shared verifier and eligibility policy — the only path a
        // candidate can take toward becoming a target.
        evaluated += 1;
        let outcome = evaluate_candidate(
            &selection,
            &CandidateInput {
                envelope_bytes: envelope.clone(),
                manifest_bytes: manifest.clone(),
            },
        );
        let result = match outcome {
            desktop_todo_update_core::CandidateOutcome::Accepted(target) => {
                // Package locator selection authority: the asset whose name
                // exactly equals the **signed** package filename. Asset names
                // are opaque — no extension or grammar filter — and an
                // ambiguous provider listing (two same-named assets) fails
                // this candidate locally instead of guessing.
                let signed_filename = target.manifest().asset().filename();
                let matches: Vec<&(String, String)> = urls
                    .assets
                    .iter()
                    .filter(|(name, _)| name == signed_filename)
                    .collect();
                let package_url = match matches.as_slice() {
                    [(_, url)] => Some(url.clone()),
                    [] => None,
                    _ => {
                        report.records.push(CandidateRecord {
                            kind: descriptor.kind,
                            release_id: descriptor.release_id.clone(),
                            tag: descriptor.tag.clone(),
                            result: CandidateResultKind::MetadataUnavailable {
                                detail: format!(
                                    "provider lists {} assets named {signed_filename:?}; ambiguous package locator",
                                    matches.len()
                                ),
                            },
                        });
                        continue;
                    }
                };
                report.records.push(CandidateRecord {
                    kind: descriptor.kind,
                    release_id: descriptor.release_id.clone(),
                    tag: descriptor.tag.clone(),
                    result: CandidateResultKind::Accepted,
                });
                return Ok(ScanResult {
                    target: Some(*target),
                    envelope_bytes: envelope,
                    package_url,
                    report,
                });
            }
            desktop_todo_update_core::CandidateOutcome::Invalid(error) => {
                CandidateResultKind::Invalid(error.kind)
            }
            desktop_todo_update_core::CandidateOutcome::Ineligible(reason) => {
                CandidateResultKind::Ineligible(reason)
            }
        };
        report.records.push(CandidateRecord {
            kind: descriptor.kind,
            release_id: descriptor.release_id.clone(),
            tag: descriptor.tag.clone(),
            result,
        });
    }

    // Whole-layer determination: the enumeration succeeded, but not one
    // candidate could be evaluated and every failure was a transport
    // failure — the discovery layer served nothing usable. This — and only
    // this — is "unavailable as a whole" for the Auto fallback contract.
    // Partial degradation (at least one candidate evaluated) completes the
    // scan instead.
    if evaluated == 0 && transport_failures > 0 {
        let error = last_transport_error.unwrap_or(FetchError::Transport {
            detail: "provider served no evaluable candidate".to_string(),
        });
        return Err((error, report));
    }
    Ok(ScanResult {
        target: None,
        envelope_bytes: Vec::new(),
        package_url: None,
        report,
    })
}

/// Fetch one metadata document. Classification of the transport result is
/// frozen-derived: content-level garbage (missing asset, oversized body,
/// off-allowlist redirect) is candidate-local and continues the scan;
/// everything else is a discovery-layer failure.
fn fetch_metadata_pair(
    ctx: &DiscoveryContext<'_>,
    urls: &providers::MetadataUrls,
    deadline: Option<Instant>,
) -> Result<(Vec<u8>, Vec<u8>), MetadataFetchError> {
    let envelope = metadata_bytes(
        ctx,
        &urls.envelope,
        BodyKind::Envelope,
        desktop_todo_update_core::ENVELOPE_MAX_BYTES,
        deadline,
    )?;
    let manifest = metadata_bytes(
        ctx,
        &urls.manifest,
        BodyKind::Manifest,
        desktop_todo_update_core::MANIFEST_MAX_BYTES,
        deadline,
    )?;
    Ok((envelope, manifest))
}

enum MetadataFetchError {
    /// A definitive, candidate-scoped answer: content-level garbage or
    /// policy violation (missing asset, oversized body, off-allowlist
    /// redirect, non-identity content encoding). The scan continues.
    CandidateLocal(CandidateResultKind),
    /// An ambiguous availability failure (timeout, connection, unexpected
    /// status, body read). Also candidate-local for the scan itself, but
    /// counted: if no candidate at all gets evaluated, the scan ends
    /// layer-unavailable instead of "no eligible candidate" — a dead
    /// network must never masquerade as "no update available".
    Transport(FetchError),
}

fn metadata_bytes(
    ctx: &DiscoveryContext<'_>,
    url: &str,
    kind: BodyKind,
    cap: usize,
    deadline: Option<Instant>,
) -> Result<Vec<u8>, MetadataFetchError> {
    match with_bounded_retry(ctx.config, deadline, |per_request_timeout| {
        fetch_bounded(ctx.client, url, cap, kind, per_request_timeout)
    }) {
        Ok(bytes) => Ok(bytes),
        Err(FetchError::HttpStatus { status: 404, .. }) => Err(MetadataFetchError::CandidateLocal(
            CandidateResultKind::MetadataUnavailable {
                detail: format!("metadata asset not found (HTTP 404): {kind:?}"),
            },
        )),
        Err(FetchError::BodyTooLarge { kind, cap }) => Err(MetadataFetchError::CandidateLocal(
            CandidateResultKind::MetadataUnavailable {
                detail: format!(
                    "metadata body exceeded the compiled {kind:?} bound of {cap} bytes"
                ),
            },
        )),
        Err(FetchError::RedirectRejected { detail }) => Err(MetadataFetchError::CandidateLocal(
            CandidateResultKind::MetadataUnavailable {
                detail: format!("metadata redirect outside the adapter allowlist: {detail}"),
            },
        )),
        Err(FetchError::ContentEncodingRejected { encoding }) => Err(
            MetadataFetchError::CandidateLocal(CandidateResultKind::MetadataUnavailable {
                detail: format!("metadata served with non-identity content encoding {encoding:?}"),
            }),
        ),
        Err(FetchError::BudgetExhausted) => Err(MetadataFetchError::CandidateLocal(
            CandidateResultKind::MetadataUnavailable {
                detail: "whole-scan budget expired during metadata fetch".to_string(),
            },
        )),
        Err(error) => Err(MetadataFetchError::Transport(error)),
    }
}

/// Run discovery for one release source.
///
/// Auto semantics (frozen): a complete bounded candidate discovery against
/// GitHub; only when the GitHub discovery layer is unavailable as a whole —
/// and no trusted target exists yet, which in this phase means the GitHub
/// scan did not accept one — does Auto switch to Gitee for candidate
/// discovery. Auto never queries both providers to compare versions and
/// never chases Gitee for a newer release after a valid-but-older GitHub
/// result: that result completes discovery. Explicit sources never silently
/// switch.
pub fn discover(source: ReleaseSource, ctx: &DiscoveryContext<'_>) -> DiscoveryOutcome {
    match source {
        ReleaseSource::GitHub | ReleaseSource::Gitee => {
            let kind = match source {
                ReleaseSource::GitHub => ProviderKind::GitHub,
                _ => ProviderKind::Gitee,
            };
            let endpoints = match endpoints_for(ctx, kind) {
                Ok(endpoints) => endpoints,
                Err(kind) => {
                    return DiscoveryOutcome::DiscoveryUnavailable {
                        provider: kind,
                        error: FetchError::Transport {
                            detail: "no compiled endpoints for the requested source".to_string(),
                        },
                        scans: Vec::new(),
                    }
                }
            };
            match scan_provider(ctx, endpoints) {
                Ok(ScanResult {
                    target: Some(target),
                    envelope_bytes,
                    package_url,
                    report,
                }) => DiscoveryOutcome::TargetFound {
                    target: Box::new(target),
                    envelope_bytes,
                    package_url,
                    scans: vec![report],
                },
                Ok(ScanResult {
                    target: None,
                    report,
                    ..
                }) => DiscoveryOutcome::NoEligibleCandidate {
                    scans: vec![report],
                },
                Err((error, report)) => DiscoveryOutcome::DiscoveryUnavailable {
                    provider: kind,
                    error,
                    scans: vec![report],
                },
            }
        }
        ReleaseSource::Auto => {
            let Some(github) = ctx
                .endpoints
                .iter()
                .find(|e| e.kind == ProviderKind::GitHub)
            else {
                return DiscoveryOutcome::DiscoveryUnavailable {
                    provider: ProviderKind::GitHub,
                    error: FetchError::Transport {
                        detail: "no compiled GitHub endpoints".to_string(),
                    },
                    scans: Vec::new(),
                };
            };
            match scan_provider(ctx, github) {
                Ok(ScanResult {
                    target: Some(target),
                    envelope_bytes,
                    package_url,
                    report,
                }) => DiscoveryOutcome::TargetFound {
                    target: Box::new(target),
                    envelope_bytes,
                    package_url,
                    scans: vec![report],
                },
                Ok(ScanResult {
                    target: None,
                    report,
                    ..
                }) => DiscoveryOutcome::NoEligibleCandidate {
                    scans: vec![report],
                },
                Err((github_error, github_report)) => {
                    // GitHub discovery layer unavailable as a whole, and no
                    // trusted target exists: the frozen Auto fallback runs a
                    // complete bounded discovery against Gitee.
                    let Some(gitee) = ctx.endpoints.iter().find(|e| e.kind == ProviderKind::Gitee)
                    else {
                        return DiscoveryOutcome::DiscoveryUnavailable {
                            provider: ProviderKind::GitHub,
                            error: github_error,
                            scans: vec![github_report],
                        };
                    };
                    match scan_provider(ctx, gitee) {
                        Ok(ScanResult {
                            target: Some(target),
                            envelope_bytes,
                            package_url,
                            report,
                        }) => DiscoveryOutcome::TargetFound {
                            target: Box::new(target),
                            envelope_bytes,
                            package_url,
                            scans: vec![github_report, report],
                        },
                        Ok(ScanResult {
                            target: None,
                            report,
                            ..
                        }) => DiscoveryOutcome::NoEligibleCandidate {
                            scans: vec![github_report, report],
                        },
                        Err((gitee_error, report)) => DiscoveryOutcome::DiscoveryUnavailable {
                            provider: ProviderKind::Gitee,
                            error: gitee_error,
                            scans: vec![github_report, report],
                        },
                    }
                }
            }
        }
    }
}
