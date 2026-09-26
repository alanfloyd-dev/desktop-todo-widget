//! Golden raw-byte signature fixtures (protocol v1, "Canonical signed
//! bytes") plus the hostile-mutation matrix.
//!
//! The committed fixture set is `tests/golden/`: `manifest-A.json` (exact
//! bytes), `envelope-A.json` (signature over those exact bytes), and
//! `key-1-public.hex` (the matching TEST-ONLY public key). The tests prove
//! that verification binds to the exact served bytes: any semantically
//! irrelevant byte change — a space, indentation, a newline, field order, an
//! EOF newline — invalidates the original signature and must fail, with no
//! repair path.
//!
//! Fixtures are deterministic: the manifest text is the frozen example from
//! protocol v1, and Ed25519 (RFC 8032) signing is deterministic, so
//! re-signing the committed manifest with the committed test seed reproduces
//! the committed signature byte-for-byte. Regenerate the committed files
//! with `cargo test -p desktop-todo-update-core --test golden
//! regenerate_golden_fixtures -- --ignored` and review the diff.

mod common;

use common::{envelope_bytes, public_raw, sign, signing_k1, signing_k2};
use desktop_todo_update_core::{derive_key_id, verify_and_parse, ErrorKind, TrustStore};

/// The golden manifest: the frozen example document from
/// docs/maintenance-protocol-v1.md, compact form, no trailing newline.
#[allow(clippy::useless_concat)] // keep the frozen example on readable raw-string lines
pub const MANIFEST_A: &str = concat!(
    r#"{"schemaVersion":1,"appId":"net.alanfloyd.desktop","channel":"stable","version":"1.2.0","publishedAt":"2026-10-01T00:00:00Z","notes":"Application lifecycle management.","updaterProtocol":1,"assets":{"windows-x64":{"filename":"desktop-todo-widget-v1.2.0-windows-x64.zip","size":3000000,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","installFiles":[{"identity":"mainExecutable","filename":"desktop-todo-widget.exe","size":5500000,"sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},{"identity":"maintenanceHelper","filename":"desktop-todo-maintenance.exe","size":800000,"sha256":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"}]}}}"#
);

fn golden_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn read_golden(name: &str) -> Vec<u8> {
    std::fs::read(golden_dir().join(name)).expect("golden fixture must exist")
}

fn write_golden(name: &str, bytes: &[u8]) {
    std::fs::write(golden_dir().join(name), bytes).expect("write golden fixture");
}

/// One-time fixture generation (`--ignored`): writes the committed golden
/// bytes. Deterministic — rerunning must produce an empty diff.
#[test]
#[ignore = "fixture regeneration; run explicitly and review the diff"]
fn regenerate_golden_fixtures() {
    let manifest_bytes = MANIFEST_A.as_bytes();
    let signature = sign(&signing_k1(), manifest_bytes);
    write_golden("manifest-A.json", manifest_bytes);
    write_golden(
        "envelope-A.json",
        &envelope_bytes(&signing_k1(), &signature),
    );
    // The raw 32-byte Ed25519 public key, lowercase hex (the keyId in the
    // envelope is derived from these bytes by the tests).
    let raw: Vec<String> = public_raw(&signing_k1())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    write_golden("key-1-public.hex", raw.concat().as_bytes());
}

#[test]
fn golden_manifest_bytes_verify() {
    let manifest_bytes = read_golden("manifest-A.json");
    let envelope = read_golden("envelope-A.json");
    let raw_key = hex_to_raw_key(&String::from_utf8(read_golden("key-1-public.hex")).unwrap());

    // The committed manifest is byte-identical to the frozen example text.
    assert_eq!(manifest_bytes, MANIFEST_A.as_bytes());

    let trust = TrustStore::from_raw_keys(&[raw_key]).unwrap();
    let target = verify_and_parse(&trust, &envelope, &manifest_bytes).unwrap();
    assert_eq!(target.manifest().version().to_string(), "1.2.0");
    assert_eq!(target.manifest_sha256_hex().len(), 64);
    assert_eq!(target.manifest().asset().install_files().len(), 2);
    // The envelope's keyId is the SHA-256 of the committed raw public key.
    let envelope_text = String::from_utf8(envelope.clone()).unwrap();
    assert!(envelope_text.contains(&derive_key_id(&raw_key)));
}

/// Decode 64 lowercase hex characters into the raw 32-byte public key.
fn hex_to_raw_key(text: &str) -> [u8; 32] {
    let mut raw = [0u8; 32];
    for (index, pair) in text.as_bytes().chunks(2).enumerate() {
        raw[index] = u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap();
    }
    raw
}

/// Regression pin (audit F-2D): the SHA-256 of the committed
/// `manifest-A.json` bytes (703 bytes), precomputed outside the crate, so
/// the golden digest assertion cannot pass by recomputing the expected
/// value with the same helper it asserts on.
const MANIFEST_A_SHA256: &str = "f1277c58b09ce66d8c03d5748bb6b4c756d55d6eeaa0ae277c3212244f1ddf1c";

#[test]
fn golden_digest_matches_independent_precomputed_sha256() {
    let manifest_bytes = read_golden("manifest-A.json");
    let envelope = read_golden("envelope-A.json");
    let raw_key = hex_to_raw_key(&String::from_utf8(read_golden("key-1-public.hex")).unwrap());
    let trust = TrustStore::from_raw_keys(&[raw_key]).unwrap();

    let target = verify_and_parse(&trust, &envelope, &manifest_bytes).unwrap();
    assert_eq!(target.manifest_sha256_hex(), MANIFEST_A_SHA256);
}

/// Regression pin (audit F-2E): the accepted target carries the exact
/// served bytes, and a byte-mutated variant can never yield a
/// `VerifiedTarget`.
#[test]
fn accepted_target_carries_the_exact_served_bytes() {
    let manifest_bytes = read_golden("manifest-A.json");
    let envelope = read_golden("envelope-A.json");
    let raw_key = hex_to_raw_key(&String::from_utf8(read_golden("key-1-public.hex")).unwrap());
    let trust = TrustStore::from_raw_keys(&[raw_key]).unwrap();

    let target = verify_and_parse(&trust, &envelope, &manifest_bytes).unwrap();
    assert_eq!(target.raw_bytes(), manifest_bytes.as_slice());

    let mut mutated = manifest_bytes.clone();
    mutated.insert(mutated.len() - 1, b' ');
    assert_ne!(mutated, manifest_bytes);
    let error = verify_and_parse(&trust, &envelope, &mutated).unwrap_err();
    assert_eq!(error.kind, ErrorKind::BadSignature);
}

#[test]
fn deterministic_signing_reproduces_the_committed_signature() {
    // Re-sign the committed manifest with the committed test seed: the
    // envelope must be byte-identical to the committed one.
    let manifest_bytes = read_golden("manifest-A.json");
    let signature = sign(&signing_k1(), &manifest_bytes);
    assert_eq!(
        envelope_bytes(&signing_k1(), &signature),
        read_golden("envelope-A.json")
    );
}

#[test]
fn signature_rejects_every_semantic_preserving_byte_mutation() {
    let manifest_bytes = read_golden("manifest-A.json");
    let envelope = read_golden("envelope-A.json");
    let trust = TrustStore::from_raw_keys(&[hex_to_raw_key(
        &String::from_utf8(read_golden("key-1-public.hex")).unwrap(),
    )])
    .unwrap();
    let text = String::from_utf8(manifest_bytes.clone()).unwrap();

    let mutations: Vec<(&str, Vec<u8>)> = vec![
        // Added space inside the document.
        (
            "added space",
            text.replace(r#""channel":"stable""#, r#""channel": "stable""#)
                .into_bytes(),
        ),
        // Added inner whitespace (record separator form).
        (
            "added inner space",
            text.replace(r#""assets":{"windows-x64""#, r#""assets":{"windows-x64" "#)
                .into_bytes(),
        ),
        // Changed field order (same JSON meaning, different bytes).
        ("changed field order", {
            let reordered = text.replacen(r#""schemaVersion":1,"#, "", 1).replace(
                r#""updaterProtocol":1,"#,
                r#""updaterProtocol":1,"schemaVersion":1,"#,
            );
            reordered.into_bytes()
        }),
        // EOF newline change.
        ("eof newline", {
            let mut bytes = manifest_bytes.clone();
            bytes.push(b'\n');
            bytes
        }),
        // Trailing space.
        ("trailing space", {
            let mut bytes = manifest_bytes.clone();
            bytes.push(b' ');
            bytes
        }),
        // Single changed hash digit (not even whitespace).
        (
            "changed value",
            text.replace(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                &format!("{}b", "a".repeat(63)),
            )
            .into_bytes(),
        ),
    ];

    for (label, mutated) in mutations {
        assert_ne!(mutated, manifest_bytes, "{label}");
        let error = verify_and_parse(&trust, &envelope, &mutated).expect_err(label);
        assert_eq!(error.kind, ErrorKind::BadSignature, "{label}");
    }
}

#[test]
fn envelope_mutations_fail_closed() {
    let manifest_bytes = read_golden("manifest-A.json");
    let raw_key = hex_to_raw_key(&String::from_utf8(read_golden("key-1-public.hex")).unwrap());
    let trust = TrustStore::from_raw_keys(&[raw_key]).unwrap();

    // Wrong key entirely (untrusted) — the second TEST-ONLY key.
    let signature = sign(&signing_k1(), &manifest_bytes);
    let other_envelope = envelope_bytes(&signing_k2(), &signature);
    assert_eq!(
        verify_and_parse(&trust, &other_envelope, &manifest_bytes)
            .unwrap_err()
            .kind,
        ErrorKind::UntrustedKey
    );

    // Signature bytes swapped to another document's signature.
    let other_manifest = common::manifest_text("1.4.0", 1, "windows-x64").into_bytes();
    let other_signature = sign(&signing_k1(), &other_manifest);
    let cross_envelope = envelope_bytes(&signing_k1(), &other_signature);
    assert_eq!(
        verify_and_parse(&trust, &cross_envelope, &manifest_bytes)
            .unwrap_err()
            .kind,
        ErrorKind::BadSignature
    );
}

#[test]
fn empty_trust_store_fails_closed_on_every_candidate() {
    let manifest_bytes = read_golden("manifest-A.json");
    let envelope = read_golden("envelope-A.json");
    let error = verify_and_parse(&TrustStore::empty(), &envelope, &manifest_bytes).unwrap_err();
    assert_eq!(error.kind, ErrorKind::UntrustedKey);
}
