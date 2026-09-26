//! Bounded candidate enumeration and the eligibility predicate (protocol v1,
//! "Candidate enumeration and eligibility").
//!
//! The freeze happens only after the complete candidate-eligibility
//! predicate succeeds, never after signature/protocol validation alone. The
//! caller supplies candidates newest-to-oldest (the provider's bounded
//! `listCandidates` order); the planner classifies each one and stops at the
//! first acceptance.
//!
//! Two distinct refusal families, both of which continue the bounded scan:
//!
//! - **candidate-ineligible** — a genuine candidate this client cannot or
//!   must not take: untrusted keyId, unsupported updaterProtocol, no
//!   applicable platform, target ≤ installed version, or an incompatible
//!   source→target rollback hop.
//! - **candidate-invalid** — a malformed or unverifiable candidate:
//!   malformed envelope/manifest, bad signature, noncanonical Base64,
//!   schema/semantic violations.
//!
//! The rationale is deliberate: the provider sits outside the trust
//! boundary, so a compromised publisher account can publish garbage as its
//! latest; letting that garbage block a still-valid, correctly signed bridge
//! would grant the provider a denial-of-service lever over the scan.

use crate::error::{ErrorKind, ProtocolError};
use crate::trust::TrustStore;
use crate::verify::{verify_and_parse, VerifiedTarget};
use crate::version::Version;

/// One candidate as exposed by a (future) ReleaseSource: the exact raw
/// bytes it serves, nothing parsed, nothing normalized.
#[derive(Debug, Clone)]
pub struct CandidateInput {
    pub envelope_bytes: Vec<u8>,
    pub manifest_bytes: Vec<u8>,
}

/// Release-gate input: whether the direct source→target hop is eligible.
/// Protocol 1 deliberately carries no wire metadata for this — Option A
/// rollback compatibility is a per-release gate fact reviewed by the
/// publisher ("the compatibility baseline is the actual installed source
/// version"), so the client compiles it as policy and the planner enforces
/// it here. A direct hop is eligible only when every durable write the
/// target runtime can perform before HealthAck is proven compatible with a
/// rollback runtime at `source`.
pub trait RollbackCompatibility {
    fn hop_eligible(&self, source: &Version, target: &Version) -> bool;
}

/// Why a genuine candidate cannot serve this client. The bounded scan
/// continues to the next-older candidate in every case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IneligibilityReason {
    UntrustedKey,
    UnsupportedUpdaterProtocol,
    PlatformMismatch,
    VersionNotNewer,
    HopIncompatible,
}

/// One candidate's outcome.
#[derive(Debug)]
pub enum CandidateOutcome {
    /// The candidate passed the complete predicate; its target is frozen.
    Accepted(Box<VerifiedTarget>),
    /// A genuine candidate this client cannot or must not take.
    Ineligible(IneligibilityReason),
    /// A malformed or unverifiable candidate.
    Invalid(ProtocolError),
}

impl CandidateOutcome {
    pub fn is_accepted(&self) -> bool {
        matches!(self, CandidateOutcome::Accepted(_))
    }

    /// The ineligibility reason, when the outcome is candidate-ineligible.
    pub fn as_ineligible(&self) -> Option<IneligibilityReason> {
        match self {
            CandidateOutcome::Ineligible(reason) => Some(*reason),
            _ => None,
        }
    }

    /// The stable error kind, when the outcome is candidate-invalid.
    pub fn invalid_kind(&self) -> Option<ErrorKind> {
        match self {
            CandidateOutcome::Invalid(error) => Some(error.kind),
            _ => None,
        }
    }
}

/// The bounded scan result: per-candidate outcomes in scan order plus the
/// index of the accepted candidate, if any.
#[derive(Debug)]
pub struct CandidateScan {
    pub outcomes: Vec<CandidateOutcome>,
    pub accepted_index: Option<usize>,
}

impl CandidateScan {
    /// Take the frozen accepted target, if the scan accepted one.
    pub fn into_accepted(self) -> Option<(usize, VerifiedTarget)> {
        let index = self.accepted_index?;
        match self.outcomes.into_iter().nth(index) {
            Some(CandidateOutcome::Accepted(target)) => Some((index, *target)),
            _ => None,
        }
    }
}

/// Client-side selection context. The installed version is the
/// proven-consistent anchor (the installed-version consistency invariant);
/// candidate selection never runs against an inconsistent evidence set.
pub struct SelectionContext<'a> {
    pub trust: &'a TrustStore,
    pub installed_version: &'a Version,
    pub hop_policy: &'a dyn RollbackCompatibility,
}

/// Classify a single candidate. The order is deterministic and mirrors the
/// frozen predicate: authorization and document validity first, then the
/// eligibility facts that depend on client state.
pub fn evaluate_candidate(
    context: &SelectionContext<'_>,
    candidate: &CandidateInput,
) -> CandidateOutcome {
    match verify_and_parse(
        context.trust,
        &candidate.envelope_bytes,
        &candidate.manifest_bytes,
    ) {
        Ok(target) => {
            // Eligibility facts, in frozen predicate order: version policy,
            // then the rollback-compatibility hop.
            if target.manifest().version() <= context.installed_version {
                CandidateOutcome::Ineligible(IneligibilityReason::VersionNotNewer)
            } else if !context
                .hop_policy
                .hop_eligible(context.installed_version, target.manifest().version())
            {
                CandidateOutcome::Ineligible(IneligibilityReason::HopIncompatible)
            } else {
                CandidateOutcome::Accepted(Box::new(target))
            }
        }
        Err(error) => match error.kind {
            // Genuine-but-unusable candidates: the scan continues.
            ErrorKind::UntrustedKey => {
                CandidateOutcome::Ineligible(IneligibilityReason::UntrustedKey)
            }
            ErrorKind::UnsupportedUpdaterProtocol => {
                CandidateOutcome::Ineligible(IneligibilityReason::UnsupportedUpdaterProtocol)
            }
            ErrorKind::PlatformMismatch => {
                CandidateOutcome::Ineligible(IneligibilityReason::PlatformMismatch)
            }
            // Malformed or unverifiable candidates: the scan also continues,
            // but under the other refusal family.
            _ => CandidateOutcome::Invalid(error),
        },
    }
}

/// Scan the bounded candidate set newest-to-oldest and freeze the first
/// candidate whose complete eligibility predicate succeeds. Never freezes on
/// signature validity alone; if nothing is eligible the result says so
/// explicitly instead of picking the newest candidate.
pub fn select_candidates(
    context: &SelectionContext<'_>,
    candidates: &[CandidateInput],
) -> CandidateScan {
    let mut outcomes = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let outcome = evaluate_candidate(context, candidate);
        let accepted = outcome.is_accepted();
        outcomes.push(outcome);
        if accepted {
            return CandidateScan {
                accepted_index: Some(outcomes.len() - 1),
                outcomes,
            };
        }
    }
    CandidateScan {
        outcomes,
        accepted_index: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trust::TrustStore;

    /// A policy that accepts only hops explicitly listed. Test stand-in for
    /// the compiled release-gate compatibility table.
    struct TableHops(Vec<(&'static str, &'static str)>);

    impl RollbackCompatibility for TableHops {
        fn hop_eligible(&self, source: &Version, target: &Version) -> bool {
            self.0.iter().any(|(s, t)| {
                Version::parse(s).unwrap() == *source && Version::parse(t).unwrap() == *target
            })
        }
    }

    #[test]
    fn empty_scan_reports_no_eligible_candidate() {
        let trust = TrustStore::empty();
        let installed = Version::parse("1.2.0").unwrap();
        let policy = TableHops(vec![]);
        let context = SelectionContext {
            trust: &trust,
            installed_version: &installed,
            hop_policy: &policy,
        };
        let scan = select_candidates(&context, &[]);
        assert!(scan.accepted_index.is_none());
        assert!(scan.outcomes.is_empty());
    }
}
