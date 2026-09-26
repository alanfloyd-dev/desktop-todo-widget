//! Required candidate-selection scenarios (Phase 2B-A section 11),
//! deterministic and fully offline.
//!
//! Candidates are always ordered newest-to-oldest, exactly as the provider's
//! bounded enumeration delivers them.

mod common;

use common::{
    candidate, derive_key_id, public_raw, sign, signing_k1, signing_k2, tampered_candidate,
    trust_with,
};
use desktop_todo_update_core::{
    evaluate_candidate, select_candidates, CandidateInput, CandidateOutcome, CandidateScan,
    ErrorKind, IneligibilityReason, RollbackCompatibility, SelectionContext, TrustStore, Version,
};

/// A compiled release-gate stand-in: only the listed source→target hops are
/// proven rollback-compatible. Scenario B's frozen example: a v1.2 source
/// can hop directly to v1.4 but not to v1.6, whose pre-HealthAck durable
/// writes are not v1.2-compatible.
struct GateHops(Vec<(&'static str, &'static str)>);

impl RollbackCompatibility for GateHops {
    fn hop_eligible(&self, source: &Version, target: &Version) -> bool {
        self.0.iter().any(|(s, t)| {
            Version::parse(s).unwrap() == *source && Version::parse(t).unwrap() == *target
        })
    }
}

fn context<'a>(
    trust: &'a TrustStore,
    installed: &'a Version,
    hops: &'a GateHops,
) -> SelectionContext<'a> {
    SelectionContext {
        trust,
        installed_version: installed,
        hop_policy: hops,
    }
}

fn expect_ineligible(outcome: &CandidateOutcome, reason: IneligibilityReason) {
    assert_eq!(outcome.as_ineligible(), Some(reason), "{outcome:?}");
}

fn expect_accepted(scan: &CandidateScan, index: usize, version: &str) {
    assert_eq!(scan.accepted_index, Some(index), "{scan:?}");
    let outcome = &scan.outcomes[index];
    assert!(outcome.is_accepted(), "{outcome:?}");
    if let CandidateOutcome::Accepted(target) = outcome {
        assert_eq!(target.manifest().version().to_string(), version);
    }
}

/// Scenario A — newest signed by an untrusted (rotation-future) key is
/// skipped; the K1-signed older release is selected.
#[test]
fn scenario_a_newest_k2_unavailable_to_k1_only_client() {
    let trust = trust_with(&[&signing_k1()]);
    let hops = GateHops(vec![("1.2.0", "1.4.0"), ("1.2.0", "1.6.0")]);
    let candidates = vec![
        candidate(&signing_k2(), "1.6.0", 1, "windows-x64"),
        candidate(&signing_k1(), "1.4.0", 1, "windows-x64"),
    ];
    let scan = select_candidates(
        &context(&trust, &Version::parse("1.2.0").unwrap(), &hops),
        &candidates,
    );
    expect_ineligible(&scan.outcomes[0], IneligibilityReason::UntrustedKey);
    expect_accepted(&scan, 1, "1.4.0");
}

/// Scenario B — newest hop unsafe against the actual installed source
/// version; the highest reachable safe next hop is selected.
#[test]
fn scenario_b_newest_hop_unsafe_routes_to_safe_bridge() {
    let trust = trust_with(&[&signing_k1()]);
    let hops = GateHops(vec![("1.2.0", "1.4.0")]);
    let candidates = vec![
        candidate(&signing_k1(), "1.6.0", 1, "windows-x64"),
        candidate(&signing_k1(), "1.4.0", 1, "windows-x64"),
    ];
    let scan = select_candidates(
        &context(&trust, &Version::parse("1.2.0").unwrap(), &hops),
        &candidates,
    );
    expect_ineligible(&scan.outcomes[0], IneligibilityReason::HopIncompatible);
    expect_accepted(&scan, 1, "1.4.0");
}

/// Scenario C — a malicious or broken latest cannot block a still-valid
/// older bridge: malformed, bad-signature, and noncanonical-Base64 variants
/// are each candidate-invalid and the scan continues.
#[test]
fn scenario_c_broken_latest_does_not_block_older_bridge() {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;

    let trust = trust_with(&[&signing_k1()]);
    let hops = GateHops(vec![("1.2.0", "1.4.0")]);

    // Malformed latest: not JSON at all.
    let malformed = CandidateInput {
        envelope_bytes: b"not json".to_vec(),
        manifest_bytes: b"also not json".to_vec(),
    };
    // Bad-signature latest: manifest bytes altered after signing.
    let bad_signature = tampered_candidate(&signing_k1(), "1.6.0");
    // Noncanonical-Base64 latest: the padding stripped from an otherwise
    // valid envelope (a tolerant decoder would still decode it).
    let good = candidate(&signing_k1(), "1.6.0", 1, "windows-x64");
    let signature = sign(&signing_k1(), &good.manifest_bytes);
    let stripped = STANDARD.encode(signature).trim_end_matches('=').to_string();
    let noncanonical = CandidateInput {
        envelope_bytes: format!(
            r#"{{"schemaVersion":1,"algorithm":"Ed25519","keyId":"{}","signature":"{stripped}"}}"#,
            derive_key_id(&public_raw(&signing_k1()))
        )
        .into_bytes(),
        manifest_bytes: good.manifest_bytes.clone(),
    };

    let expected_kind = |label: &str| match label {
        "malformed" => ErrorKind::EnvelopeMalformed,
        "bad signature" => ErrorKind::BadSignature,
        "noncanonical base64" => ErrorKind::SignatureEncoding,
        _ => unreachable!(),
    };

    for (label, latest) in [
        ("malformed", malformed),
        ("bad signature", bad_signature),
        ("noncanonical base64", noncanonical),
    ] {
        let candidates = vec![latest, candidate(&signing_k1(), "1.4.0", 1, "windows-x64")];
        let scan = select_candidates(
            &context(&trust, &Version::parse("1.2.0").unwrap(), &hops),
            &candidates,
        );
        assert_eq!(
            scan.outcomes[0].invalid_kind(),
            Some(expected_kind(label)),
            "{label}"
        );
        expect_accepted(&scan, 1, "1.4.0");
    }
}

/// Scenario D — equal and lower targets are skipped as up-to-date/downgrade.
#[test]
fn scenario_d_not_newer_candidates_are_skipped() {
    let trust = trust_with(&[&signing_k1()]);
    let hops = GateHops(vec![("1.2.0", "1.3.0")]);
    let candidates = vec![
        candidate(&signing_k1(), "1.2.0", 1, "windows-x64"), // equal
        candidate(&signing_k1(), "1.1.0", 1, "windows-x64"), // downgrade
        candidate(&signing_k1(), "1.3.0", 1, "windows-x64"), // eligible
    ];
    let scan = select_candidates(
        &context(&trust, &Version::parse("1.2.0").unwrap(), &hops),
        &candidates,
    );
    expect_ineligible(&scan.outcomes[0], IneligibilityReason::VersionNotNewer);
    expect_ineligible(&scan.outcomes[1], IneligibilityReason::VersionNotNewer);
    expect_accepted(&scan, 2, "1.3.0");
}

/// Scenario E — a candidate with no windows-x64 asset is ineligible.
#[test]
fn scenario_e_wrong_platform_is_skipped() {
    let trust = trust_with(&[&signing_k1()]);
    let hops = GateHops(vec![("1.2.0", "1.4.0"), ("1.2.0", "1.3.0")]);
    let candidates = vec![
        candidate(&signing_k1(), "1.4.0", 1, "linux-x64"),
        candidate(&signing_k1(), "1.3.0", 1, "windows-x64"),
    ];
    let scan = select_candidates(
        &context(&trust, &Version::parse("1.2.0").unwrap(), &hops),
        &candidates,
    );
    expect_ineligible(&scan.outcomes[0], IneligibilityReason::PlatformMismatch);
    expect_accepted(&scan, 1, "1.3.0");
}

/// Scenario F — an unsupported updaterProtocol is ineligible, and the scan
/// continues to an executable older release.
#[test]
fn scenario_f_unsupported_updater_protocol_is_skipped() {
    let trust = trust_with(&[&signing_k1()]);
    let hops = GateHops(vec![("1.2.0", "1.4.0"), ("1.2.0", "1.3.0")]);
    let candidates = vec![
        candidate(&signing_k1(), "1.4.0", 2, "windows-x64"),
        candidate(&signing_k1(), "1.3.0", 1, "windows-x64"),
    ];
    let scan = select_candidates(
        &context(&trust, &Version::parse("1.2.0").unwrap(), &hops),
        &candidates,
    );
    expect_ineligible(
        &scan.outcomes[0],
        IneligibilityReason::UnsupportedUpdaterProtocol,
    );
    expect_accepted(&scan, 1, "1.3.0");
}

/// Scenario G — when nothing is eligible the scan accepts nothing, instead
/// of arbitrarily picking the newest candidate.
#[test]
fn scenario_g_no_eligible_candidate() {
    let trust = trust_with(&[&signing_k1()]);
    let hops = GateHops(vec![("1.2.0", "1.4.0")]);
    let candidates = vec![
        candidate(&signing_k2(), "1.6.0", 1, "windows-x64"), // untrusted key
        candidate(&signing_k1(), "1.4.0", 2, "windows-x64"), // protocol 2
        candidate(&signing_k1(), "1.2.0", 1, "windows-x64"), // not newer
        tampered_candidate(&signing_k1(), "1.1.9"),          // invalid bytes
    ];
    let scan = select_candidates(
        &context(&trust, &Version::parse("1.2.0").unwrap(), &hops),
        &candidates,
    );
    assert_eq!(scan.accepted_index, None, "{scan:?}");
    expect_ineligible(&scan.outcomes[0], IneligibilityReason::UntrustedKey);
    expect_ineligible(
        &scan.outcomes[1],
        IneligibilityReason::UnsupportedUpdaterProtocol,
    );
    expect_ineligible(&scan.outcomes[2], IneligibilityReason::VersionNotNewer);
    assert_eq!(
        scan.outcomes[3].invalid_kind(),
        Some(ErrorKind::BadSignature)
    );
}

/// The freeze happens only after the complete predicate: a candidate that
/// merely verifies is not accepted while the hop gate still refuses it.
#[test]
fn signature_success_alone_never_freezes_the_target() {
    let trust = trust_with(&[&signing_k1()]);
    let empty_hops = GateHops(vec![]);
    let target = candidate(&signing_k1(), "1.6.0", 1, "windows-x64");
    let outcome = evaluate_candidate(
        &context(&trust, &Version::parse("1.2.0").unwrap(), &empty_hops),
        &target,
    );
    expect_ineligible(&outcome, IneligibilityReason::HopIncompatible);
}

/// Key-rotation bridge reachability (protocol v1 frozen example): the client
/// at vN trusting only K1 skips the K2-signed vN+2 and takes the K1-signed
/// bridge vN+1 whose runtime compiles the K1+K2 store; after restarting
/// under the bridge it installs the K2-signed release through normal
/// selection.
#[test]
fn key_rotation_bridge_is_reachable_through_normal_selection() {
    let k1_only = trust_with(&[&signing_k1()]);
    let hops = GateHops(vec![("1.2.0", "1.3.0")]);
    let candidates = vec![
        candidate(&signing_k2(), "1.4.0", 1, "windows-x64"),
        candidate(&signing_k1(), "1.3.0", 1, "windows-x64"),
    ];
    let scan = select_candidates(
        &context(&k1_only, &Version::parse("1.2.0").unwrap(), &hops),
        &candidates,
    );
    expect_accepted(&scan, 1, "1.3.0");

    let both_keys = trust_with(&[&signing_k1(), &signing_k2()]);
    let hops = GateHops(vec![("1.3.0", "1.4.0")]);
    let candidates = vec![candidate(&signing_k2(), "1.4.0", 1, "windows-x64")];
    let scan = select_candidates(
        &context(&both_keys, &Version::parse("1.3.0").unwrap(), &hops),
        &candidates,
    );
    expect_accepted(&scan, 0, "1.4.0");
}

/// Structured per-candidate kinds stay distinguishable for diagnostics: a
/// well-formed-but-unknown keyId is UntrustedKey, a malformed one fails the
/// envelope shape before any lookup.
#[test]
fn per_candidate_kinds_stay_distinguishable() {
    let trust = trust_with(&[&signing_k1()]);
    let hops = GateHops(vec![]);

    // Well-formed keyId absent from the store.
    let unknown_key = candidate(&signing_k2(), "1.4.0", 1, "windows-x64");
    let outcome = evaluate_candidate(
        &context(&trust, &Version::parse("1.2.0").unwrap(), &hops),
        &unknown_key,
    );
    assert_eq!(
        outcome.as_ineligible(),
        Some(IneligibilityReason::UntrustedKey)
    );

    // Malformed keyId shape never reaches the trust store.
    let good = candidate(&signing_k1(), "1.4.0", 1, "windows-x64");
    let envelope = String::from_utf8(good.envelope_bytes)
        .unwrap()
        .replace(&derive_key_id(&public_raw(&signing_k1())), &"g".repeat(64));
    let outcome = evaluate_candidate(
        &context(&trust, &Version::parse("1.2.0").unwrap(), &hops),
        &CandidateInput {
            envelope_bytes: envelope.into_bytes(),
            manifest_bytes: good.manifest_bytes,
        },
    );
    assert_eq!(outcome.invalid_kind(), Some(ErrorKind::KeyIdMalformed));
}
