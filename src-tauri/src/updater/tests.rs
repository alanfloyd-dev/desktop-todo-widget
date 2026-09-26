//! Deterministic, fully offline tests for the discovery layer. A minimal
//! in-process HTTP server (std TcpListener, no mocking framework) controls
//! status codes, headers, chunking, body sizes, redirects, ordering, and
//! malformed responses. No live github.com/gitee.com access, and no test
//! feature swaps production URLs — tests inject mock endpoints explicitly
//! through the same parameters production code uses.
//!
//! **TEST-ONLY Ed25519 seeds.** Never production trust roots, never used for
//! release signing.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use desktop_todo_update_core::{
    derive_key_id, IneligibilityReason, RollbackCompatibility, TrustStore, Version,
};
use ed25519_dalek::{Signer, SigningKey};

use super::*;
use crate::updater::http_fetch::{discovery_client, fetch_bounded, BodyKind, FetchError};
use crate::updater::providers::{
    combined_origin_allowlist, enumerate_candidates, ProviderEndpoints, ProviderKind,
    ENVELOPE_ASSET_NAME, MANIFEST_ASSET_NAME,
};

const TEST_SEED_K1: [u8; 32] = *b"dtw-test-KEY1-ONLY-not-for-relea";
const TEST_SEED_K2: [u8; 32] = *b"dtw-test-KEY2-ONLY-not-for-relea";

fn signing_k1() -> SigningKey {
    SigningKey::from_bytes(&TEST_SEED_K1)
}

fn signing_k2() -> SigningKey {
    SigningKey::from_bytes(&TEST_SEED_K2)
}

struct GateHops(Vec<(&'static str, &'static str)>);

impl RollbackCompatibility for GateHops {
    fn hop_eligible(&self, source: &Version, target: &Version) -> bool {
        self.0.iter().any(|(s, t)| {
            Version::parse(s).unwrap() == *source && Version::parse(t).unwrap() == *target
        })
    }
}

/// A protocol-valid manifest document (synthetic hashes only).
fn manifest_text(version: &str, updater_protocol: u32) -> String {
    let hash = "a".repeat(64);
    format!(
        concat!(
            r#"{{"schemaVersion":1,"appId":"net.alanfloyd.desktop","channel":"stable","version":"{v}","#,
            r#""publishedAt":"2026-10-01T00:00:00Z","notes":"Application lifecycle management.","updaterProtocol":{p},"#,
            r#""assets":{{"windows-x64":{{"filename":"desktop-todo-widget-v{v}-windows-x64.zip","size":3000000,"sha256":"{hash}","installFiles":["#,
            r#"{{"identity":"mainExecutable","filename":"desktop-todo-widget.exe","size":5500000,"sha256":"{hash}"}},"#,
            r#"{{"identity":"maintenanceHelper","filename":"desktop-todo-maintenance.exe","size":800000,"sha256":"{hash}"}}]}}}}}}"#
        ),
        v = version,
        p = updater_protocol,
        hash = hash
    )
}

fn sign(key: &SigningKey, bytes: &[u8]) -> [u8; 64] {
    key.sign(bytes).to_bytes()
}

fn envelope_json(key: &SigningKey, signature: &[u8; 64]) -> String {
    format!(
        r#"{{"schemaVersion":1,"algorithm":"Ed25519","keyId":"{}","signature":"{}"}}"#,
        derive_key_id(&key.verifying_key().to_bytes()),
        STANDARD.encode(signature)
    )
}

// ---------------------------------------------------------------- mock server

/// Serve canned responses from an in-process HTTP server. `Connection:
/// close` on every response keeps each request on its own connection, so a
/// stateless per-connection thread is enough. The bound address is passed to
/// the handler so responses can reference real asset URLs.
type MockHandler = Arc<dyn Fn(&str, SocketAddr) -> Vec<u8> + Send + Sync>;

fn spawn_mock(handler: MockHandler) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let handler = handler.clone();
            std::thread::spawn(move || {
                // Read the request head: loop until the CRLFCRLF terminator
                // so a request split across packets is still fully consumed.
                let mut head = Vec::new();
                let mut chunk = [0u8; 1024];
                loop {
                    let Ok(n) = stream.read(&mut chunk) else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    head.extend_from_slice(&chunk[..n]);
                    if head.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&head).to_string();
                let target = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                let response = handler(&target, addr);
                let _ = stream.write_all(&response);
                let _ = stream.flush();
            });
        }
    });
    addr
}

/// Handler adapter: the mock hands each response builder the bound address.
fn serve(handler: impl Fn(&str, SocketAddr) -> Vec<u8> + Send + Sync + 'static) -> SocketAddr {
    spawn_mock(Arc::new(handler))
}

fn http_ok(body: &[u8]) -> Vec<u8> {
    http_status("HTTP/1.1 200 OK", &[], body)
}

fn http_status(status: &str, extra: &[(&str, String)], body: &[u8]) -> Vec<u8> {
    let mut response = format!("{status}\r\nConnection: close\r\n");
    for (name, value) in extra {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    let mut bytes = response.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

fn chunked_response(status: &str, chunks: &[&[u8]]) -> Vec<u8> {
    let mut bytes =
        format!("{status}\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n\r\n").into_bytes();
    for chunk in chunks {
        bytes.extend(format!("{:x}\r\n", chunk.len()).into_bytes());
        bytes.extend_from_slice(chunk);
        bytes.extend(b"\r\n");
    }
    bytes.extend(b"0\r\n\r\n");
    bytes
}

fn not_found() -> Vec<u8> {
    http_status("HTTP/1.1 404 Not Found", &[], b"missing")
}

// ---------------------------------------------------------------- fixtures

fn test_config() -> DiscoveryConfig {
    DiscoveryConfig {
        max_candidates: 10,
        provider_index_body_cap: 1024 * 1024,
        connect_timeout: Duration::from_secs(2),
        request_timeout: Duration::from_secs(2),
        max_retries: 2,
        retry_backoff: Duration::ZERO,
        retry_after_cap: Duration::ZERO,
        max_redirects: 4,
    }
}

fn test_endpoints(addr: SocketAddr) -> Vec<ProviderEndpoints> {
    let base = format!("http://{addr}");
    vec![
        ProviderEndpoints {
            kind: ProviderKind::GitHub,
            releases_url: format!("{base}/github/releases?per_page={{bound}}"),
            download_origin_allowlist: vec![base.clone()],
        },
        ProviderEndpoints {
            kind: ProviderKind::Gitee,
            releases_url: format!("{base}/gitee/releases?page=1&per_page={{bound}}"),
            download_origin_allowlist: vec![base.clone()],
        },
    ]
}

fn release_json(
    id: u64,
    tag: &str,
    draft: bool,
    prerelease: bool,
    assets: &[(&str, &str)],
) -> String {
    let assets_json: Vec<String> = assets
        .iter()
        .map(|(name, url)| format!(r#"{{"name":"{name}","browser_download_url":"{url}"}}"#))
        .collect();
    format!(
        r#"{{"id":{id},"tag_name":"{tag}","draft":{draft},"prerelease":{prerelease},"published_at":"2026-10-01T00:00:00Z","assets":[{}]}}"#,
        assets_json.join(",")
    )
}

/// The two metadata asset entries for one release directory.
fn asset_entries(addr: SocketAddr, dir: &str) -> [(String, String); 2] {
    [
        (
            ENVELOPE_ASSET_NAME.to_string(),
            format!("http://{addr}/download/{dir}/{ENVELOPE_ASSET_NAME}"),
        ),
        (
            MANIFEST_ASSET_NAME.to_string(),
            format!("http://{addr}/download/{dir}/{MANIFEST_ASSET_NAME}"),
        ),
    ]
}

fn release_with_assets(addr: SocketAddr, id: u64, dir: &str, version: &str) -> String {
    let entries = asset_entries(addr, dir);
    release_json(
        id,
        &format!("v{version}"),
        false,
        false,
        &[
            (entries[0].0.as_str(), entries[0].1.as_str()),
            (entries[1].0.as_str(), entries[1].1.as_str()),
        ],
    )
}

/// Signed metadata bytes for one candidate: (envelope bytes, manifest bytes)
/// with the signature over the exact manifest bytes.
fn signed_metadata(key: &SigningKey, version: &str, updater_protocol: u32) -> (Vec<u8>, Vec<u8>) {
    let manifest = manifest_text(version, updater_protocol);
    let signature = sign(key, manifest.as_bytes());
    let envelope = envelope_json(key, &signature).into_bytes();
    (envelope, manifest.into_bytes())
}

/// Serve signed metadata bytes for a candidate directory, or 404 when the
/// test did not stage that path (which doubles as the missing-asset
/// scenario).
fn serve_candidate_bytes(
    target: &str,
    dir: &str,
    version: &str,
    updater_protocol: u32,
    key: &SigningKey,
) -> Option<Vec<u8>> {
    let (envelope, manifest) = signed_metadata(key, version, updater_protocol);
    if target == format!("/download/{dir}/{ENVELOPE_ASSET_NAME}") {
        return Some(http_ok(&envelope));
    }
    if target == format!("/download/{dir}/{MANIFEST_ASSET_NAME}") {
        return Some(http_ok(&manifest));
    }
    None
}

struct Fixture {
    client: reqwest::blocking::Client,
    config: DiscoveryConfig,
    endpoints: Vec<ProviderEndpoints>,
    trust: TrustStore,
    installed: Version,
    hop: GateHops,
}

fn fixture(addr: SocketAddr) -> Fixture {
    let config = test_config();
    let endpoints = test_endpoints(addr);
    let client = discovery_client(&config, &combined_origin_allowlist(&endpoints)).unwrap();
    Fixture {
        client,
        config,
        endpoints,
        trust: TrustStore::from_raw_keys(&[signing_k1().verifying_key().to_bytes()]).unwrap(),
        installed: Version::parse("1.2.0").unwrap(),
        hop: GateHops(vec![("1.2.0", "1.4.0"), ("1.2.0", "1.3.0")]),
    }
}

fn run(fixture: &Fixture, source: ReleaseSource) -> DiscoveryOutcome {
    let ctx = DiscoveryContext {
        config: &fixture.config,
        client: &fixture.client,
        endpoints: &fixture.endpoints,
        trust: &fixture.trust,
        installed_version: &fixture.installed,
        hop_policy: &fixture.hop,
    };
    discover(source, &ctx)
}

fn assert_target(outcome: DiscoveryOutcome, expected_version: &str) -> VerifiedTarget {
    match outcome {
        DiscoveryOutcome::TargetFound { target, .. } => {
            assert_eq!(target.manifest().version().to_string(), expected_version);
            *target
        }
        other => panic!("expected TargetFound {expected_version}, got {other:?}"),
    }
}

// ---------------------------------------------------------------- A. GitHub

/// Bounded enumeration: the provider is asked for exactly the compiled bound
/// and never more, the bound caps the descriptor list, and the provider's
/// newest-first order is preserved as scan order.
#[test]
fn enumeration_is_bounded_and_provider_ordered() {
    let hits = Arc::new(AtomicUsize::new(0));
    let seen_query = Arc::new(std::sync::Mutex::new(String::new()));
    // Provider order is newest-first: build the list accordingly.
    let releases: Vec<String> = (0..12)
        .rev()
        .map(|i| release_json(i, &format!("v1.9.{i}"), false, false, &[]))
        .collect();
    let body = format!("[{}]", releases.join(","));
    let hits_clone = hits.clone();
    let seen_clone = seen_query.clone();
    let addr = serve(move |target, _| {
        hits_clone.fetch_add(1, Ordering::SeqCst);
        if target.starts_with("/github/releases") {
            *seen_clone.lock().unwrap() = target.to_string();
            return http_ok(body.as_bytes());
        }
        not_found()
    });

    let config = test_config();
    let endpoints = test_endpoints(addr);
    let client = discovery_client(&config, &combined_origin_allowlist(&endpoints)).unwrap();
    let enumeration = enumerate_candidates(&client, &endpoints[0], &config).unwrap();

    assert_eq!(*seen_query.lock().unwrap(), "/github/releases?per_page=10");
    assert_eq!(enumeration.candidates.len(), 10);
    // Provider order preserved newest-to-oldest, no re-sorting anywhere.
    assert_eq!(enumeration.candidates[0].tag, "v1.9.11");
    assert_eq!(enumeration.candidates[9].tag, "v1.9.2");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

/// Draft and prerelease releases are not publication-eligible for protocol
/// 1's stable delivery and never enter the candidate set; the scan continues
/// past them to the published stable release.
#[test]
fn draft_and_prerelease_are_not_publication_eligible() {
    let addr = serve(|target, addr| {
        if target.starts_with("/github/releases") {
            let bridge = release_with_assets(addr, 1, "bridge", "1.4.0");
            let list = format!(
                "[{},{},{}]",
                release_json(3, "v1.6.0", true, false, &[]),
                release_json(2, "v1.5.0-beta.1", false, true, &[]),
                bridge
            );
            return http_ok(list.as_bytes());
        }
        serve_candidate_bytes(target, "bridge", "1.4.0", 1, &signing_k1()).unwrap_or_else(not_found)
    });
    let fixture = fixture(addr);
    assert_target(run(&fixture, ReleaseSource::GitHub), "1.4.0");
}

/// A release the provider published without the envelope or without the
/// manifest is candidate-local garbage: the scan continues to the older
/// valid bridge.
#[test]
fn missing_metadata_assets_are_candidate_local() {
    let addr = serve(|target, addr| {
        if target.starts_with("/github/releases") {
            // Newest release carries only the manifest asset (envelope
            // missing); the bridge is complete.
            let manifest_url = format!("http://{addr}/download/newest/{MANIFEST_ASSET_NAME}");
            let newest = release_json(
                9,
                "v1.6.0",
                false,
                false,
                &[(MANIFEST_ASSET_NAME, manifest_url.as_str())],
            );
            let bridge = release_with_assets(addr, 2, "bridge", "1.4.0");
            return http_ok(format!("[{newest},{bridge}]").as_bytes());
        }
        if target == format!("/download/newest/{MANIFEST_ASSET_NAME}") {
            return http_ok(manifest_text("1.6.0", 1).as_bytes());
        }
        serve_candidate_bytes(target, "bridge", "1.4.0", 1, &signing_k1()).unwrap_or_else(not_found)
    });
    let fixture = fixture(addr);
    let target = assert_target(run(&fixture, ReleaseSource::GitHub), "1.4.0");
    assert_eq!(
        target.manifest().version().to_string(),
        "1.4.0",
        "newest garbage must not block the bridge"
    );
}

/// A malformed provider releases response is a discovery-layer failure,
/// never a candidate verdict: an explicit source reports it, and Auto falls
/// back to Gitee per the frozen policy.
#[test]
fn malformed_provider_json_is_discovery_unavailable() {
    let addr = serve(|target, addr| {
        if target.starts_with("/github/releases") {
            return http_ok(b"this is not json");
        }
        if target.starts_with("/gitee/releases") {
            let bridge = release_with_assets(addr, 1, "bridge", "1.4.0");
            return http_ok(format!("[{bridge}]").as_bytes());
        }
        serve_candidate_bytes(target, "bridge", "1.4.0", 1, &signing_k1()).unwrap_or_else(not_found)
    });
    let fixture = fixture(addr);
    match run(&fixture, ReleaseSource::GitHub) {
        DiscoveryOutcome::DiscoveryUnavailable { provider, .. } => {
            assert_eq!(provider, ProviderKind::GitHub)
        }
        other => panic!("expected DiscoveryUnavailable, got {other:?}"),
    }
    // Auto: the GitHub layer is unavailable as a whole and no target exists,
    // so the frozen fallback completes discovery on Gitee.
    assert_target(run(&fixture, ReleaseSource::Auto), "1.4.0");
}

/// A non-transient non-success status after the bounded retries is a
/// discovery-layer failure for the explicit source, with the frozen retry
/// shape (initial attempt plus two retries).
#[test]
fn persistent_http_failure_is_discovery_unavailable_after_bounded_retries() {
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = hits.clone();
    let addr = serve(move |target, _| {
        if target.starts_with("/github/releases") {
            hits_clone.fetch_add(1, Ordering::SeqCst);
            return http_status("HTTP/1.1 500 Internal Server Error", &[], b"boom");
        }
        not_found()
    });
    let fixture = fixture(addr);
    match run(&fixture, ReleaseSource::GitHub) {
        DiscoveryOutcome::DiscoveryUnavailable {
            provider,
            error: FetchError::HttpStatus { status: 500, .. },
            ..
        } => assert_eq!(provider, ProviderKind::GitHub),
        other => panic!("expected DiscoveryUnavailable(500), got {other:?}"),
    }
    assert_eq!(hits.load(Ordering::SeqCst), 3);
}

/// A rate-limited discovery request honors bounded Retry-After, retries,
/// and recovers on the third attempt.
#[test]
fn rate_limit_respects_bounded_retry_after_then_recovers() {
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = hits.clone();
    let addr = serve(move |target, _| {
        if target.starts_with("/github/releases") {
            let n = hits_clone.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                return http_status(
                    "HTTP/1.1 429 Too Many Requests",
                    &[("Retry-After", "60".to_string())],
                    b"slow down",
                );
            }
            return http_ok(b"[]");
        }
        not_found()
    });
    let config = test_config();
    let endpoints = test_endpoints(addr);
    // retry_after_cap = 0 (test config): bounded Retry-After is clamped to
    // the cap, so the test stays fast while the header is still honored.
    let client = discovery_client(&config, &combined_origin_allowlist(&endpoints)).unwrap();
    let enumeration = enumerate_candidates(&client, &endpoints[0], &config).unwrap();
    assert_eq!(enumeration.candidates.len(), 0);
    assert_eq!(hits.load(Ordering::SeqCst), 3);
}

// ---------------------------------------------------------------- C. bounds

/// Envelope and manifest bodies are bounded before the full body is read:
/// exactly-at-limit accepted, limit+1 rejected.
#[test]
fn metadata_body_bounds_are_enforced_at_the_boundary() {
    let envelope_cap = desktop_todo_update_core::ENVELOPE_MAX_BYTES;
    let manifest_cap = desktop_todo_update_core::MANIFEST_MAX_BYTES;
    let mut routes: HashMap<String, Vec<u8>> = HashMap::new();
    routes.insert("/env/at".to_string(), http_ok(&vec![b'a'; envelope_cap]));
    routes.insert(
        "/env/over".to_string(),
        http_ok(&vec![b'a'; envelope_cap + 1]),
    );
    routes.insert("/man/at".to_string(), http_ok(&vec![b'b'; manifest_cap]));
    routes.insert(
        "/man/over".to_string(),
        http_ok(&vec![b'b'; manifest_cap + 1]),
    );
    let addr = serve(move |target, _| match routes.get(target) {
        Some(response) => response.clone(),
        None => not_found(),
    });
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    assert_eq!(
        fetch_bounded(
            &client,
            &format!("http://{addr}/env/at"),
            envelope_cap,
            BodyKind::Envelope
        )
        .unwrap()
        .len(),
        envelope_cap
    );
    assert!(matches!(
        fetch_bounded(
            &client,
            &format!("http://{addr}/env/over"),
            envelope_cap,
            BodyKind::Envelope
        ),
        Err(FetchError::BodyTooLarge { .. })
    ));
    assert_eq!(
        fetch_bounded(
            &client,
            &format!("http://{addr}/man/at"),
            manifest_cap,
            BodyKind::Manifest
        )
        .unwrap()
        .len(),
        manifest_cap
    );
    assert!(matches!(
        fetch_bounded(
            &client,
            &format!("http://{addr}/man/over"),
            manifest_cap,
            BodyKind::Manifest
        ),
        Err(FetchError::BodyTooLarge { .. })
    ));
}

/// Missing Content-Length (chunked) is still bounded and the exact bytes
/// arrive unmodified; an oversized chunked body is rejected at the hard
/// cap + 1 ceiling, never read to unbounded completion.
#[test]
fn chunked_bodies_are_bounded_and_exact() {
    let envelope_cap = desktop_todo_update_core::ENVELOPE_MAX_BYTES;
    let small = vec![b'x'; 100];
    let oversized = vec![b'y'; envelope_cap + 100];
    let mut routes: HashMap<String, Vec<u8>> = HashMap::new();
    routes.insert(
        "/chunk/small".to_string(),
        chunked_response("HTTP/1.1 200 OK", &[&small[..40], &small[40..]]),
    );
    routes.insert(
        "/chunk/over".to_string(),
        chunked_response("HTTP/1.1 200 OK", &[&oversized[..2000], &oversized[2000..]]),
    );
    let addr = serve(move |target, _| match routes.get(target) {
        Some(response) => response.clone(),
        None => not_found(),
    });
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    let body = fetch_bounded(
        &client,
        &format!("http://{addr}/chunk/small"),
        envelope_cap,
        BodyKind::Envelope,
    )
    .unwrap();
    assert_eq!(body, small, "exact bytes, no trim or normalization");
    assert!(matches!(
        fetch_bounded(
            &client,
            &format!("http://{addr}/chunk/over"),
            envelope_cap,
            BodyKind::Envelope
        ),
        Err(FetchError::BodyTooLarge { .. })
    ));
}

/// A lying small Content-Length can never expand the read: the fetched body
/// is at most the declared length (or a bounded read failure), never more.
#[test]
fn lying_small_content_length_is_still_bounded() {
    // Hand-built response: declares Content-Length: 10 but sends 4096 bytes.
    let mut response =
        b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 10\r\n\r\n".to_vec();
    response.extend(vec![b'z'; 4096]);
    let mut routes: HashMap<String, Vec<u8>> = HashMap::new();
    routes.insert("/lie".to_string(), response);
    let addr = serve(move |target, _| match routes.get(target) {
        Some(response) => response.clone(),
        None => not_found(),
    });
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    match fetch_bounded(
        &client,
        &format!("http://{addr}/lie"),
        4096,
        BodyKind::Envelope,
    ) {
        Ok(body) => assert!(body.len() <= 10, "read must respect the declared length"),
        Err(FetchError::BodyRead { .. }) | Err(FetchError::Transport { .. }) => {}
        other => panic!("unexpected outcome: {other:?}"),
    }
}

// ---------------------------------------------------------------- D. bytes

/// The bytes that reach the verifier are the exact served bytes: internal
/// whitespace and the EOF newline survive, so a manifest signed over those
/// exact bytes verifies through the full network path.
#[test]
fn exact_bytes_reach_the_verifier_unchanged() {
    let addr = serve(|target, addr| {
        if target.starts_with("/github/releases") {
            return http_ok(format!("[{}]", release_with_assets(addr, 1, "1", "1.4.0")).as_bytes());
        }
        if target == format!("/download/1/{ENVELOPE_ASSET_NAME}") {
            // Signature covers the whitespace-mutated bytes served below.
            let mut manifest = manifest_text("1.4.0", 1).into_bytes();
            manifest.insert(manifest.len() - 1, b' ');
            manifest.push(b'\n');
            let signature = sign(&signing_k1(), &manifest);
            return http_ok(envelope_json(&signing_k1(), &signature).as_bytes());
        }
        if target == format!("/download/1/{MANIFEST_ASSET_NAME}") {
            let mut manifest = manifest_text("1.4.0", 1).into_bytes();
            manifest.insert(manifest.len() - 1, b' ');
            manifest.push(b'\n');
            return http_ok(&manifest);
        }
        not_found()
    });
    let fixture = fixture(addr);
    let target = assert_target(run(&fixture, ReleaseSource::GitHub), "1.4.0");
    let mut expected = manifest_text("1.4.0", 1).into_bytes();
    expected.insert(expected.len() - 1, b' ');
    expected.push(b'\n');
    assert_eq!(target.raw_bytes(), expected.as_slice());
}

// ---------------------------------------------------------------- E. bridge

/// The frozen bridge scenarios over the real network path: a newest
/// candidate that is garbage (bad signature, unknown key, unsupported
/// protocol) never blocks the older valid bridge.
#[test]
fn newest_garbage_never_blocks_the_older_bridge() {
    for scenario in ["bad-signature", "unknown-key", "unsupported-protocol"] {
        let scenario = scenario.to_string();
        let addr = serve(move |target, addr| {
            if target.starts_with("/github/releases") {
                let newest = release_with_assets(addr, 1, "newest", "1.6.0");
                let bridge = release_with_assets(addr, 2, "bridge", "1.4.0");
                return http_ok(format!("[{newest},{bridge}]").as_bytes());
            }
            if target.contains("/download/newest/") {
                let (envelope, manifest) = match scenario.as_str() {
                    // Signature over different bytes than served.
                    "bad-signature" => {
                        let manifest = manifest_text("1.6.0", 1);
                        let signature = sign(&signing_k1(), manifest.as_bytes());
                        let mut tampered = manifest.into_bytes();
                        tampered.insert(tampered.len() - 1, b' ');
                        (
                            envelope_json(&signing_k1(), &signature).into_bytes(),
                            tampered,
                        )
                    }
                    // Signed by the rotation-future K2 the client does not trust.
                    "unknown-key" => signed_metadata(&signing_k2(), "1.6.0", 1),
                    // Declares an updaterProtocol this client cannot execute.
                    "unsupported-protocol" => signed_metadata(&signing_k1(), "1.6.0", 2),
                    other => unreachable!("{other}"),
                };
                return if target.ends_with(ENVELOPE_ASSET_NAME) {
                    http_ok(&envelope)
                } else {
                    http_ok(&manifest)
                };
            }
            serve_candidate_bytes(target, "bridge", "1.4.0", 1, &signing_k1())
                .unwrap_or_else(not_found)
        });
        let fixture = fixture(addr);
        assert_target(run(&fixture, ReleaseSource::GitHub), "1.4.0");
    }
}

/// When nothing is eligible the result is the explicit no-target outcome,
/// with every candidate's structured verdict in the scan report.
#[test]
fn no_eligible_candidate_is_explicit() {
    let addr = serve(|target, addr| {
        if target.starts_with("/github/releases") {
            let k2 = release_with_assets(addr, 3, "k2", "1.6.0");
            let equal = release_with_assets(addr, 4, "equal", "1.2.0");
            return http_ok(format!("[{k2},{equal}]").as_bytes());
        }
        if target.contains("/download/k2/") {
            let (envelope, manifest) = signed_metadata(&signing_k2(), "1.6.0", 1);
            return if target.ends_with(ENVELOPE_ASSET_NAME) {
                http_ok(&envelope)
            } else {
                http_ok(&manifest)
            };
        }
        if target.contains("/download/equal/") {
            let (envelope, manifest) = signed_metadata(&signing_k1(), "1.2.0", 1);
            return if target.ends_with(ENVELOPE_ASSET_NAME) {
                http_ok(&envelope)
            } else {
                http_ok(&manifest)
            };
        }
        not_found()
    });
    let fixture = fixture(addr);
    match run(&fixture, ReleaseSource::GitHub) {
        DiscoveryOutcome::NoEligibleCandidate { scans } => {
            let records = &scans[0].records;
            assert_eq!(
                records[0].result,
                CandidateResultKind::Ineligible(IneligibilityReason::UntrustedKey)
            );
            assert_eq!(
                records[1].result,
                CandidateResultKind::Ineligible(IneligibilityReason::VersionNotNewer)
            );
        }
        other => panic!("expected NoEligibleCandidate, got {other:?}"),
    }
}

// ------------------------------------------------------- F/G. separation

/// Provider identity never affects trust: the identical signed candidate
/// served by GitHub and by Gitee produces the identical verification result.
#[test]
fn provider_identity_does_not_affect_trust() {
    let addr = serve(|target, addr| {
        if target.starts_with("/github/releases") {
            return http_ok(format!("[{}]", release_with_assets(addr, 1, "1", "1.4.0")).as_bytes());
        }
        if target.starts_with("/gitee/releases") {
            return http_ok(format!("[{}]", release_with_assets(addr, 7, "1", "1.4.0")).as_bytes());
        }
        serve_candidate_bytes(target, "1", "1.4.0", 1, &signing_k1()).unwrap_or_else(not_found)
    });
    let fixture = fixture(addr);

    let github_target = assert_target(run(&fixture, ReleaseSource::GitHub), "1.4.0");
    let gitee_target = assert_target(run(&fixture, ReleaseSource::Gitee), "1.4.0");
    assert_eq!(
        github_target.manifest_sha256_hex(),
        gitee_target.manifest_sha256_hex()
    );
    assert_eq!(github_target.raw_bytes(), gitee_target.raw_bytes());
}

/// With the production trust store still empty (no provisioned release key),
/// every network candidate is untrusted and the discovery chain stays
/// fail-closed: no outcome can ever become a target.
#[test]
fn empty_production_store_never_trusts_network_candidates() {
    let addr = serve(|target, addr| {
        if target.starts_with("/github/releases") {
            return http_ok(format!("[{}]", release_with_assets(addr, 1, "1", "1.4.0")).as_bytes());
        }
        serve_candidate_bytes(target, "1", "1.4.0", 1, &signing_k1()).unwrap_or_else(not_found)
    });
    let mut fixture = fixture(addr);
    fixture.trust = TrustStore::empty();
    match run(&fixture, ReleaseSource::GitHub) {
        DiscoveryOutcome::NoEligibleCandidate { scans } => {
            assert!(matches!(
                scans[0].records[0].result,
                CandidateResultKind::Ineligible(IneligibilityReason::UntrustedKey)
            ));
        }
        other => panic!("empty store must fail closed, got {other:?}"),
    }
}

// ------------------------------------------------- redirects and timeout

/// An allowlisted same-origin redirect is followed; a redirect to a
/// non-allowlisted origin is rejected as candidate-local garbage and never
/// blocks the older bridge.
#[test]
fn redirects_stay_inside_the_origin_allowlist() {
    let evil_hits = Arc::new(AtomicUsize::new(0));
    let evil_hits_clone = evil_hits.clone();
    let addr = serve(move |target, addr| {
        if target.starts_with("/github/releases") {
            let newest = release_with_assets(addr, 1, "newest", "1.6.0");
            let bridge = release_with_assets(addr, 2, "bridge", "1.4.0");
            return http_ok(format!("[{newest},{bridge}]").as_bytes());
        }
        // Newest metadata redirects off-allowlist: rejected, never fetched.
        if target.starts_with("/download/newest/") {
            return http_status(
                "HTTP/1.1 302 Found",
                &[("Location", "https://evil.example/manifest".to_string())],
                b"",
            );
        }
        // The bridge envelope redirects once inside the allowlist and is
        // then served from the redirected path.
        if target == format!("/download/bridge/{ENVELOPE_ASSET_NAME}") {
            return http_status(
                "HTTP/1.1 302 Found",
                &[("Location", format!("http://{addr}/download/bridge/actual"))],
                b"",
            );
        }
        if target == "/download/bridge/actual" {
            let (envelope, _) = signed_metadata(&signing_k1(), "1.4.0", 1);
            return http_ok(&envelope);
        }
        if target == format!("/download/bridge/{MANIFEST_ASSET_NAME}") {
            let (_, manifest) = signed_metadata(&signing_k1(), "1.4.0", 1);
            return http_ok(&manifest);
        }
        if target.contains("evil") {
            evil_hits_clone.fetch_add(1, Ordering::SeqCst);
        }
        not_found()
    });
    let fixture = fixture(addr);
    let target = assert_target(run(&fixture, ReleaseSource::GitHub), "1.4.0");
    assert_eq!(target.manifest().version().to_string(), "1.4.0");
    assert_eq!(
        evil_hits.load(Ordering::SeqCst),
        0,
        "off-allowlist redirect target must never be fetched"
    );
}

/// An unresponsive discovery layer surfaces as a bounded-finite Timeout
/// after the frozen retry shape — never as a candidate verdict, and never
/// an unbounded hang.
#[test]
fn unresponsive_provider_times_out_bounded() {
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = hits.clone();
    let addr = serve(move |target, _| {
        hits_clone.fetch_add(1, Ordering::SeqCst);
        if target.starts_with("/github/releases") {
            std::thread::sleep(Duration::from_secs(2));
            return http_ok(b"[]");
        }
        not_found()
    });
    let mut fixture = fixture(addr);
    fixture.config.request_timeout = Duration::from_millis(300);
    fixture.client = discovery_client(
        &fixture.config,
        &combined_origin_allowlist(&fixture.endpoints),
    )
    .unwrap();
    let started = Instant::now();
    match run(&fixture, ReleaseSource::GitHub) {
        DiscoveryOutcome::DiscoveryUnavailable {
            provider,
            error: FetchError::Timeout { .. },
            ..
        } => assert_eq!(provider, ProviderKind::GitHub),
        other => panic!("expected bounded Timeout, got {other:?}"),
    }
    // Three attempts (initial + two retries), each bounded at 300 ms.
    assert_eq!(hits.load(Ordering::SeqCst), 3);
    assert!(started.elapsed() < Duration::from_secs(5));
}

// ------------------------------------------------------------------ Auto

/// Auto's frozen fallback: GitHub layer unavailable → complete bounded
/// Gitee discovery; and a valid-but-older GitHub result completes discovery
/// without chasing Gitee for a newer one.
#[test]
fn auto_falls_back_only_when_the_github_layer_is_unavailable() {
    // Case 1: GitHub layer unavailable → Gitee discovery runs and finds the
    // bridge.
    let github_hits = Arc::new(AtomicUsize::new(0));
    let github_hits_clone = github_hits.clone();
    let addr = serve(move |target, addr| {
        if target.starts_with("/github/releases") {
            github_hits_clone.fetch_add(1, Ordering::SeqCst);
            return http_status("HTTP/1.1 500 Internal Server Error", &[], b"down");
        }
        if target.starts_with("/gitee/releases") {
            let bridge = release_with_assets(addr, 2, "bridge", "1.4.0");
            return http_ok(format!("[{bridge}]").as_bytes());
        }
        serve_candidate_bytes(target, "bridge", "1.4.0", 1, &signing_k1()).unwrap_or_else(not_found)
    });
    let fixture_a = fixture(addr);
    let target = assert_target(run(&fixture_a, ReleaseSource::Auto), "1.4.0");
    assert_eq!(target.manifest().version().to_string(), "1.4.0");
    assert_eq!(github_hits.load(Ordering::SeqCst), 3);

    // Case 2: GitHub answers with a valid-but-older eligible candidate →
    // discovery completes on GitHub; Gitee is never queried for a newer one.
    let gitee_hits = Arc::new(AtomicUsize::new(0));
    let gitee_hits_clone = gitee_hits.clone();
    let addr = serve(move |target, addr| {
        if target.starts_with("/github/releases") {
            let older = release_with_assets(addr, 5, "older", "1.3.0");
            return http_ok(format!("[{older}]").as_bytes());
        }
        if target.starts_with("/gitee/") {
            gitee_hits_clone.fetch_add(1, Ordering::SeqCst);
        }
        serve_candidate_bytes(target, "older", "1.3.0", 1, &signing_k1()).unwrap_or_else(not_found)
    });
    let fixture_b = fixture(addr);
    assert_target(run(&fixture_b, ReleaseSource::Auto), "1.3.0");
    assert_eq!(gitee_hits.load(Ordering::SeqCst), 0);
}
// --------------------------------------------- content-encoding boundary

/// Spawn the mock and also record every raw request head, for assertions on
/// what the discovery client actually sends.
fn serve_logged(
    handler: impl Fn(&str, SocketAddr) -> Vec<u8> + Send + Sync + 'static,
) -> (SocketAddr, Arc<std::sync::Mutex<Vec<String>>>) {
    let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
    let requests_clone = requests.clone();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handler = Arc::new(handler) as Arc<dyn Fn(&str, SocketAddr) -> Vec<u8> + Send + Sync>;
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let handler = handler.clone();
            let requests_inner = requests_clone.clone();
            std::thread::spawn(move || {
                let mut head = Vec::new();
                let mut chunk = [0u8; 1024];
                loop {
                    let Ok(n) = stream.read(&mut chunk) else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    head.extend_from_slice(&chunk[..n]);
                    if head.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&head).to_string();
                requests_inner.lock().unwrap().push(request.clone());
                let target = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                let response = handler(&target, addr);
                let _ = stream.write_all(&response);
                let _ = stream.flush();
            });
        }
    });
    (addr, requests)
}

/// A: every metadata request explicitly asks for the unencoded
/// representation (Accept-Encoding: identity), so no transparent content
/// transformation can stand between the source and the verifier.
/// C: a served `Content-Encoding: identity` is accepted and the bytes stay
/// exact. B/E (header absent, newline and internal whitespace preserved)
/// are covered by `exact_bytes_reach_the_verifier_unchanged` and
/// `chunked_bodies_are_bounded_and_exact`.
#[test]
fn metadata_requests_ask_for_identity() {
    let (addr, requests) = serve_logged(|target, _addr| {
        if target.starts_with("/probe") {
            return http_ok(b"[]");
        }
        not_found()
    });
    let config = test_config();
    let client = discovery_client(&config, &[format!("http://{addr}")]).unwrap();
    fetch_bounded(
        &client,
        &format!("http://{addr}/probe"),
        1024,
        BodyKind::Envelope,
    )
    .unwrap();

    let heads = requests.lock().unwrap().clone();
    assert!(!heads.is_empty());
    assert!(
        heads.iter().all(|head| head
            .to_ascii_lowercase()
            .contains("accept-encoding: identity")),
        "every discovery request must carry Accept-Encoding: identity"
    );
}

#[test]
fn identity_content_encoding_is_accepted_with_exact_bytes() {
    let addr = serve(|target, addr| {
        if target.starts_with("/github/releases") {
            return http_ok(format!("[{}]", release_with_assets(addr, 1, "1", "1.4.0")).as_bytes());
        }
        if target == format!("/download/1/{ENVELOPE_ASSET_NAME}") {
            let (envelope, _) = signed_metadata(&signing_k1(), "1.4.0", 1);
            return http_status(
                "HTTP/1.1 200 OK",
                &[("Content-Encoding", "identity".to_string())],
                &envelope,
            );
        }
        if target == format!("/download/1/{MANIFEST_ASSET_NAME}") {
            let (_, manifest) = signed_metadata(&signing_k1(), "1.4.0", 1);
            return http_status(
                "HTTP/1.1 200 OK",
                &[("Content-Encoding", "identity".to_string())],
                &manifest,
            );
        }
        not_found()
    });
    let fixture = fixture(addr);
    let target = assert_target(run(&fixture, ReleaseSource::GitHub), "1.4.0");
    assert_eq!(
        target.raw_bytes(),
        manifest_text("1.4.0", 1).as_bytes(),
        "identity encoding must preserve the bytes exactly"
    );
}

/// D: a non-identity content encoding is rejected fail closed at the
/// transport layer — a candidate-local garbage skip that continues to the
/// older bridge, never a cryptographic verdict.
#[test]
fn non_identity_content_encoding_fails_closed_before_the_verifier() {
    let addr = serve(|target, addr| {
        if target.starts_with("/github/releases") {
            let newest = release_with_assets(addr, 1, "newest", "1.6.0");
            let bridge = release_with_assets(addr, 2, "bridge", "1.4.0");
            return http_ok(format!("[{newest},{bridge}]").as_bytes());
        }
        if target.starts_with("/download/newest/") {
            // Arbitrary body behind a gzip marker: must never reach the
            // verifier, let alone classify as BadSignature.
            return http_status(
                "HTTP/1.1 200 OK",
                &[("Content-Encoding", "gzip".to_string())],
                b"\x1f\x8b not really gzip but the transport must not care",
            );
        }
        serve_candidate_bytes(target, "bridge", "1.4.0", 1, &signing_k1()).unwrap_or_else(not_found)
    });
    let fixture = fixture(addr);
    let target = assert_target(run(&fixture, ReleaseSource::GitHub), "1.4.0");
    assert_eq!(target.manifest().version().to_string(), "1.4.0");
}

// ------------------------------------------------- transport semantics

/// T1 — derived semantics: a candidate whose metadata fetch fails at
/// transport level (timeout, after the bounded retry ladder) rejects only
/// itself; the older complete bridge is still accepted.
#[test]
fn t1_candidate_transport_failure_continues_to_the_older_bridge() {
    let addr = serve(|target, addr| {
        if target.starts_with("/github/releases") {
            let newest = release_with_assets(addr, 1, "newest", "1.6.0");
            let bridge = release_with_assets(addr, 2, "bridge", "1.4.0");
            return http_ok(format!("[{newest},{bridge}]").as_bytes());
        }
        if target.starts_with("/download/newest/") {
            // Unresponsive: every attempt times out.
            std::thread::sleep(Duration::from_secs(2));
            return http_ok(b"late");
        }
        serve_candidate_bytes(target, "bridge", "1.4.0", 1, &signing_k1()).unwrap_or_else(not_found)
    });
    let mut fixture = fixture(addr);
    fixture.config.request_timeout = Duration::from_millis(300);
    fixture.client = discovery_client(
        &fixture.config,
        &combined_origin_allowlist(&fixture.endpoints),
    )
    .unwrap();
    assert_target(run(&fixture, ReleaseSource::GitHub), "1.4.0");
}

/// T2 — derived semantics: when the enumeration succeeds but not a single
/// candidate can be evaluated (every metadata fetch fails at transport
/// level), the scan ends DiscoveryUnavailable — a dead network must never
/// masquerade as "no update available", and Auto's frozen fallback applies.
#[test]
fn t2_zero_evaluated_candidates_ends_layer_unavailable() {
    let github_hits = Arc::new(AtomicUsize::new(0));
    let github_hits_clone = github_hits.clone();
    let addr = serve(move |target, addr| {
        if target.starts_with("/github/releases") {
            github_hits_clone.fetch_add(1, Ordering::SeqCst);
            return http_ok(
                format!("[{}]", release_with_assets(addr, 1, "newest", "1.6.0")).as_bytes(),
            );
        }
        if target.starts_with("/download/newest/") {
            std::thread::sleep(Duration::from_secs(2));
            return http_ok(b"late");
        }
        if target.starts_with("/gitee/releases") {
            let bridge = release_with_assets(addr, 2, "bridge", "1.4.0");
            return http_ok(format!("[{bridge}]").as_bytes());
        }
        serve_candidate_bytes(target, "bridge", "1.4.0", 1, &signing_k1()).unwrap_or_else(not_found)
    });
    let mut fixture = fixture(addr);
    fixture.config.request_timeout = Duration::from_millis(300);
    fixture.client = discovery_client(
        &fixture.config,
        &combined_origin_allowlist(&fixture.endpoints),
    )
    .unwrap();

    // Explicit source: bounded transport failure surfaced as such.
    match run(&fixture, ReleaseSource::GitHub) {
        DiscoveryOutcome::DiscoveryUnavailable {
            provider,
            error: FetchError::Timeout { .. },
            ..
        } => assert_eq!(provider, ProviderKind::GitHub),
        other => panic!("expected DiscoveryUnavailable(Timeout), got {other:?}"),
    }

    // Auto: the layer was unavailable as a whole and no target exists, so
    // the frozen fallback runs the complete Gitee discovery.
    assert_target(run(&fixture, ReleaseSource::Auto), "1.4.0");
}

/// T3 — derived semantics: one candidate transport-failed, another was
/// evaluated (ineligible), no target: the layer demonstrably worked, so the
/// scan completes as NoEligibleCandidate (partial degradation, recorded)
/// and Auto does NOT fall back.
#[test]
fn t3_partial_degradation_completes_without_fallback() {
    let gitee_hits = Arc::new(AtomicUsize::new(0));
    let gitee_hits_clone = gitee_hits.clone();
    let addr = serve(move |target, addr| {
        if target.starts_with("/github/releases") {
            let newest = release_with_assets(addr, 1, "newest", "1.6.0");
            let k2 = release_with_assets(addr, 2, "k2", "1.4.0");
            return http_ok(format!("[{newest},{k2}]").as_bytes());
        }
        if target.starts_with("/download/newest/") {
            std::thread::sleep(Duration::from_secs(2));
            return http_ok(b"late");
        }
        if target.contains("/download/k2/") {
            let (envelope, manifest) = signed_metadata(&signing_k2(), "1.4.0", 1);
            return if target.ends_with(ENVELOPE_ASSET_NAME) {
                http_ok(&envelope)
            } else {
                http_ok(&manifest)
            };
        }
        if target.starts_with("/gitee/") {
            gitee_hits_clone.fetch_add(1, Ordering::SeqCst);
        }
        not_found()
    });
    let mut fixture = fixture(addr);
    fixture.config.request_timeout = Duration::from_millis(300);
    fixture.client = discovery_client(
        &fixture.config,
        &combined_origin_allowlist(&fixture.endpoints),
    )
    .unwrap();
    match run(&fixture, ReleaseSource::Auto) {
        DiscoveryOutcome::NoEligibleCandidate { scans } => {
            let records = &scans[0].records;
            assert!(matches!(
                records[0].result,
                CandidateResultKind::MetadataUnavailable { .. }
            ));
            assert_eq!(
                records[1].result,
                CandidateResultKind::Ineligible(IneligibilityReason::UntrustedKey)
            );
        }
        other => panic!("expected NoEligibleCandidate, got {other:?}"),
    }
    assert_eq!(
        gitee_hits.load(Ordering::SeqCst),
        0,
        "partial degradation completes discovery; no fallback"
    );
}
