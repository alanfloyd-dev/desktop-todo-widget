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
        scan_budget: None,
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
        scan_started: Instant::now(),
        client: &fixture.client,
        endpoints: &fixture.endpoints,
        trust: &fixture.trust,
        installed_version: &fixture.installed,
        hop_policy: &fixture.hop,
    };
    discover(source, &ctx)
}

fn installed_anchor() -> Version {
    Version::parse("1.2.0").unwrap()
}

fn test_trust_store() -> TrustStore {
    TrustStore::from_raw_keys(&[signing_k1().verifying_key().to_bytes()]).unwrap()
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
    let enumeration = enumerate_candidates(&client, &endpoints[0], &config, None).unwrap();

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
    let enumeration = enumerate_candidates(&client, &endpoints[0], &config, None).unwrap();
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
            BodyKind::Envelope,
            std::time::Duration::from_secs(5)
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
            BodyKind::Envelope,
            std::time::Duration::from_secs(5)
        ),
        Err(FetchError::BodyTooLarge { .. })
    ));
    assert_eq!(
        fetch_bounded(
            &client,
            &format!("http://{addr}/man/at"),
            manifest_cap,
            BodyKind::Manifest,
            std::time::Duration::from_secs(5)
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
            BodyKind::Manifest,
            std::time::Duration::from_secs(5)
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
        std::time::Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(body, small, "exact bytes, no trim or normalization");
    assert!(matches!(
        fetch_bounded(
            &client,
            &format!("http://{addr}/chunk/over"),
            envelope_cap,
            BodyKind::Envelope,
            std::time::Duration::from_secs(5)
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
        std::time::Duration::from_secs(5),
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
        std::time::Duration::from_secs(5),
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

// =================================================== Phase 2C-A: acquisition

use crate::updater::acquisition::{
    acquire_and_stage, persist_trusted_target, recover_session, AcquisitionError, MilestoneState,
    ENVELOPE_FILE, MANIFEST_FILE, PACKAGE_DOWNLOADING_FILE, PACKAGE_FILE, RECORD_FILE, STAGED_DIR,
};
use desktop_todo_maintenance::package_zip::ArchiveError;
use std::path::PathBuf;

/// Deterministic managed-executable payloads (synthetic bytes, never real
/// product binaries).
fn exe_payload(tag: u8) -> Vec<u8> {
    let mut bytes = vec![tag; 600];
    bytes.extend((0..400u32).map(|i| (i % 251) as u8));
    bytes
}

/// Build a ZIP in memory with Stored/Deflated entries.
fn zip_bytes(entries: &[(&str, Vec<u8>)], method: zip::CompressionMethod) -> Vec<u8> {
    let cursor = std::io::Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    let options = zip::write::SimpleFileOptions::default().compression_method(method);
    for (name, bytes) in entries {
        writer
            .start_file(*name, options)
            .expect("zip fixture entry");
        std::io::Write::write_all(&mut writer, bytes).expect("zip fixture body");
    }
    writer.finish().expect("zip fixture finish").into_inner()
}

fn sha_of(bytes: &[u8]) -> String {
    desktop_todo_update_core::sha256_hex(bytes)
}

/// A manifest whose package and installFiles facts match a real fixture ZIP.
fn pipeline_manifest(version: &str, package: &[u8], exe1: &[u8], exe2: &[u8]) -> String {
    format!(
        concat!(
            r#"{{"schemaVersion":1,"appId":"net.alanfloyd.desktop","channel":"stable","version":"{v}","#,
            r#""publishedAt":"2026-10-01T00:00:00Z","notes":"Application lifecycle management.","updaterProtocol":1,"#,
            r#""assets":{{"windows-x64":{{"filename":"desktop-todo-widget-v{v}-windows-x64.zip","size":{pkg},"sha256":"{psha}","installFiles":["#,
            r#"{{"identity":"mainExecutable","filename":"desktop-todo-widget.exe","size":{s1},"sha256":"{h1}"}},"#,
            r#"{{"identity":"maintenanceHelper","filename":"desktop-todo-maintenance.exe","size":{s2},"sha256":"{h2}"}}]}}}}}}"#
        ),
        v = version,
        pkg = package.len(),
        psha = sha_of(package),
        s1 = exe1.len(),
        h1 = sha_of(exe1),
        s2 = exe2.len(),
        h2 = sha_of(exe2),
    )
}

/// The full valid package: exactly the nine allowlisted root files.
fn valid_package(exe1: &[u8], exe2: &[u8]) -> Vec<u8> {
    let readme = b"readme".to_vec();
    zip_bytes(
        &[
            ("desktop-todo-widget.exe", exe1.to_vec()),
            ("desktop-todo-maintenance.exe", exe2.to_vec()),
            ("install.ps1", readme.clone()),
            ("uninstall.ps1", readme.clone()),
            ("README.md", readme.clone()),
            ("README_ZH.md", readme.clone()),
            ("LICENSE", readme.clone()),
            ("LICENSE_ZH.md", readme.clone()),
            ("THIRD_PARTY_NOTICES.md", readme),
        ],
        zip::CompressionMethod::Deflated,
    )
}

struct AcquisitionFixture {
    addr: SocketAddr,
    updates_root: PathBuf,
    session_id: String,
}

fn temp_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dtw-2ca-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("temp root");
    dir
}

/// End-to-end: discovery (mock) → persist trusted target → bounded package
/// download → exact hash → archive validation → staged managed EXEs → typed
/// PackageStaged state. The seven support files are validated as package
/// members but never staged.
#[test]
fn full_pipeline_trusted_target_to_staged_package() {
    let exe1 = exe_payload(1);
    let exe2 = exe_payload(2);
    let package = valid_package(&exe1, &exe2);
    let manifest = pipeline_manifest("1.4.0", &package, &exe1, &exe2);
    let manifest_bytes = manifest.clone().into_bytes();
    let signature = sign(&signing_k1(), manifest.as_bytes());
    let envelope = envelope_json(&signing_k1(), &signature).into_bytes();
    let envelope_bytes_fixture = envelope.clone();

    let addr = serve(move |target, addr| {
        if target.starts_with("/github/releases") {
            let release = release_json(
                1,
                "v1.4.0",
                false,
                false,
                &[
                    (
                        ENVELOPE_ASSET_NAME,
                        &format!("http://{addr}/download/update-manifest.json.sig"),
                    ),
                    (
                        MANIFEST_ASSET_NAME,
                        &format!("http://{addr}/download/update-manifest.json"),
                    ),
                    (
                        "desktop-todo-widget-v1.4.0-windows-x64.zip",
                        &format!(
                            "http://{addr}/download/desktop-todo-widget-v1.4.0-windows-x64.zip"
                        ),
                    ),
                ],
            );
            return http_ok(format!("[{release}]").as_bytes());
        }
        if target == "/download/update-manifest.json.sig" {
            return http_ok(&envelope_bytes_fixture);
        }
        if target == "/download/update-manifest.json" {
            return http_ok(&manifest_bytes);
        }
        if target.ends_with(".zip") {
            return http_ok(&package);
        }
        not_found()
    });
    let fixture = fixture(addr);
    let outcome = run(&fixture, ReleaseSource::GitHub);
    let (target, envelope_bytes, package_url) = match outcome {
        DiscoveryOutcome::TargetFound {
            target,
            envelope_bytes,
            package_url,
            ..
        } => (target, envelope_bytes, package_url),
        other => panic!("expected TargetFound, got {other:?}"),
    };
    let package_url = package_url.expect("package locator");

    let root = temp_root("pipeline");
    let expected_digest = target.manifest_sha256_hex().to_string();
    let mut session = persist_trusted_target(
        &root,
        *target,
        &envelope_bytes,
        Some(&package_url),
        "GitHub",
        &fixture.installed,
    )
    .expect("persist trusted target");
    assert_eq!(session.record.state, MilestoneState::TrustedTargetPersisted);
    assert_eq!(session.record.target_version, "1.4.0");
    assert_eq!(
        session.record.manifest_sha256, expected_digest,
        "the record binds the digest of the exact raw manifest bytes"
    );

    acquire_and_stage(&mut session, &fixture.client).expect("acquire and stage");
    assert_eq!(session.record.state, MilestoneState::PackageStaged);

    let staged_dir = session.dir.join(STAGED_DIR);
    let staged: Vec<String> = std::fs::read_dir(&staged_dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    let mut sorted = staged.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        vec![
            "desktop-todo-maintenance.exe".to_string(),
            "desktop-todo-widget.exe".to_string()
        ],
        "only the two managed executables are staged; support files are not"
    );
    assert_eq!(
        std::fs::read(staged_dir.join("desktop-todo-widget.exe")).unwrap(),
        exe1
    );
    assert_eq!(
        std::fs::read(staged_dir.join("desktop-todo-maintenance.exe")).unwrap(),
        exe2
    );
    // Raw signed bytes are durable and byte-exact.
    assert_eq!(
        std::fs::read(session.dir.join(MANIFEST_FILE)).unwrap(),
        manifest.as_bytes()
    );
    assert_eq!(
        std::fs::read(session.dir.join(ENVELOPE_FILE)).unwrap(),
        envelope
    );
    assert!(session.dir.join(PACKAGE_FILE).exists());
    assert!(!session.dir.join(PACKAGE_DOWNLOADING_FILE).exists());

    let _ = std::fs::remove_dir_all(&root);
}

/// Package locator must name the signed package file exactly; provider
/// display metadata and frontend arguments play no part.
#[test]
fn package_url_must_match_the_signed_package_filename() {
    let exe1 = exe_payload(1);
    let exe2 = exe_payload(2);
    let package = valid_package(&exe1, &exe2);
    let manifest = pipeline_manifest("1.4.0", &package, &exe1, &exe2);
    let signature = sign(&signing_k1(), manifest.as_bytes());
    let envelope = envelope_json(&signing_k1(), &signature).into_bytes();

    let addr = serve(move |target, addr| {
        if target.starts_with("/github/releases") {
            let release = release_json(
                1,
                "v1.4.0",
                false,
                false,
                &[
                    (
                        ENVELOPE_ASSET_NAME,
                        &format!("http://{addr}/download/update-manifest.json.sig"),
                    ),
                    (
                        MANIFEST_ASSET_NAME,
                        &format!("http://{addr}/download/update-manifest.json"),
                    ),
                    (
                        "some-other-name.zip",
                        &format!("http://{addr}/download/some-other-name.zip"),
                    ),
                ],
            );
            return http_ok(format!("[{release}]").as_bytes());
        }
        if target == "/download/update-manifest.json.sig" {
            return http_ok(&envelope);
        }
        if target == "/download/update-manifest.json" {
            return http_ok(manifest.as_bytes());
        }
        not_found()
    });
    let fixture = fixture(addr);
    let (target, envelope_bytes, _) = match run(&fixture, ReleaseSource::GitHub) {
        DiscoveryOutcome::TargetFound {
            target,
            envelope_bytes,
            package_url,
            ..
        } => (target, envelope_bytes, package_url),
        other => panic!("expected TargetFound, got {other:?}"),
    };
    let root = temp_root("url-mismatch");
    let mut session = persist_trusted_target(
        &root,
        *target,
        &envelope_bytes,
        Some("http://127.0.0.1:1/download/some-other-name.zip"),
        "GitHub",
        &fixture.installed,
    )
    .expect("persist");
    match acquire_and_stage(&mut session, &fixture.client) {
        Err(AcquisitionError::PackageUrlRejected { .. }) => {}
        other => panic!("expected PackageUrlRejected, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// Package body bounds and exact identity: declared size and signed SHA-256
/// are checked against the actual bytes before anything is staged; partial
/// files never become ready states.
#[test]
fn package_body_is_verified_before_staging() {
    let exe1 = exe_payload(1);
    let exe2 = exe_payload(2);
    let package = valid_package(&exe1, &exe2);

    // Size mismatch: fewer bytes than declared.
    let manifest = pipeline_manifest("1.4.0", &package, &exe1, &exe2);
    let signature = sign(&signing_k1(), manifest.as_bytes());
    let envelope = envelope_json(&signing_k1(), &signature).into_bytes();
    let truncated = package[..package.len() - 10].to_vec();
    let addr = serve(move |target, addr| {
        if target.starts_with("/github/releases") {
            let release = release_json(
                1,
                "v1.4.0",
                false,
                false,
                &[
                    (
                        ENVELOPE_ASSET_NAME,
                        &format!("http://{addr}/download/{ENVELOPE_ASSET_NAME}"),
                    ),
                    (
                        MANIFEST_ASSET_NAME,
                        &format!("http://{addr}/download/{MANIFEST_ASSET_NAME}"),
                    ),
                    (
                        "desktop-todo-widget-v1.4.0-windows-x64.zip",
                        &format!(
                            "http://{addr}/download/desktop-todo-widget-v1.4.0-windows-x64.zip"
                        ),
                    ),
                ],
            );
            return http_ok(format!("[{release}]").as_bytes());
        }
        if target.ends_with(ENVELOPE_ASSET_NAME) {
            return http_ok(&envelope);
        }
        if target.ends_with(MANIFEST_ASSET_NAME) {
            return http_ok(manifest.as_bytes());
        }
        if target.ends_with(".zip") {
            // Truncated body: stream ends early.
            return http_ok(&truncated);
        }
        not_found()
    });
    let fixture_size = fixture(addr);
    let (target, envelope_bytes, _) = match run(&fixture_size, ReleaseSource::GitHub) {
        DiscoveryOutcome::TargetFound {
            target,
            envelope_bytes,
            ..
        } => (target, envelope_bytes, None::<String>),
        other => panic!("expected TargetFound, got {other:?}"),
    };
    let root = temp_root("size");
    let mut session = persist_trusted_target(
        &root,
        *target,
        &envelope_bytes,
        Some(&format!(
            "http://{addr}/download/desktop-todo-widget-v1.4.0-windows-x64.zip"
        )),
        "GitHub",
        &fixture_size.installed,
    )
    .expect("persist");
    match acquire_and_stage(&mut session, &fixture_size.client) {
        Err(AcquisitionError::SizeMismatch { .. }) => {}
        other => panic!("expected SizeMismatch, got {other:?}"),
    }
    assert_eq!(
        session.record.state,
        MilestoneState::TrustedTargetPersisted,
        "failure must not advance the typed state"
    );
    assert!(!session.dir.join(PACKAGE_FILE).exists());
    assert!(!session.dir.join(STAGED_DIR).exists());
    let _ = std::fs::remove_dir_all(&root);

    // Hash mismatch: full length, one flipped byte.
    let mut corrupted = package.clone();
    let last = corrupted.len() - 1;
    corrupted[last] ^= 0xFF;
    let manifest = pipeline_manifest("1.4.0", &corrupted, &exe1, &exe2);
    let manifest_bytes = manifest.clone().into_bytes();
    let signature = sign(&signing_k1(), manifest.as_bytes());
    let envelope = envelope_json(&signing_k1(), &signature).into_bytes();
    // The manifest signs the corrupted bytes; serve the ORIGINAL bytes so
    // the downloaded hash disagrees with the signed digest.
    let package_fixture = package.clone();
    let addr = serve(move |target, addr| {
        if target.starts_with("/github/releases") {
            let release = release_json(
                1,
                "v1.4.0",
                false,
                false,
                &[
                    (
                        ENVELOPE_ASSET_NAME,
                        &format!("http://{addr}/download/{ENVELOPE_ASSET_NAME}"),
                    ),
                    (
                        MANIFEST_ASSET_NAME,
                        &format!("http://{addr}/download/{MANIFEST_ASSET_NAME}"),
                    ),
                    (
                        "desktop-todo-widget-v1.4.0-windows-x64.zip",
                        &format!(
                            "http://{addr}/download/desktop-todo-widget-v1.4.0-windows-x64.zip"
                        ),
                    ),
                ],
            );
            return http_ok(format!("[{release}]").as_bytes());
        }
        if target.ends_with(ENVELOPE_ASSET_NAME) {
            return http_ok(&envelope);
        }
        if target.ends_with(MANIFEST_ASSET_NAME) {
            return http_ok(&manifest_bytes);
        }
        if target.ends_with(".zip") {
            return http_ok(&package_fixture);
        }
        not_found()
    });
    let fixture_hash = fixture(addr);
    let (target, envelope_bytes, _) = match run(&fixture_hash, ReleaseSource::GitHub) {
        DiscoveryOutcome::TargetFound {
            target,
            envelope_bytes,
            ..
        } => (target, envelope_bytes, None::<String>),
        other => panic!("expected TargetFound, got {other:?}"),
    };
    let root = temp_root("hash");
    let mut session = persist_trusted_target(
        &root,
        *target,
        &envelope_bytes,
        Some(&format!(
            "http://{addr}/download/desktop-todo-widget-v1.4.0-windows-x64.zip"
        )),
        "GitHub",
        &fixture_hash.installed,
    )
    .expect("persist");
    // Serve the original (non-corrupted) bytes against the corrupted hash:
    // reuse a fresh fixture is overkill; instead corrupt the served body by
    // serving package (original) while the record expects corrupted hash.
    let _ = std::fs::create_dir_all(&session.dir);
    match acquire_and_stage(&mut session, &fixture_hash.client) {
        Err(AcquisitionError::PackageHashMismatch { .. }) => {}
        other => panic!("expected PackageHashMismatch, got {other:?}"),
    }
    assert!(!session.dir.join(PACKAGE_FILE).exists());
    assert!(!session.dir.join(STAGED_DIR).exists());
    let _ = std::fs::remove_dir_all(&root);
}

/// Streaming hard cap: the manifest declares a package beyond the compiled
/// 256 MiB cap, so the transport rejects before any body arrives; a declared
/// size within budget but served over-long is cut at declared + 1.
#[test]
fn package_streaming_bounds() {
    let exe1 = exe_payload(1);
    let exe2 = exe_payload(2);
    let package = valid_package(&exe1, &exe2);
    // The signed manifest declares the true package size; the server
    // over-serves beyond it. The streaming read is hard-capped at
    // declared + 1 and the byte count must match the declaration exactly.
    let manifest = pipeline_manifest("1.4.0", &package, &exe1, &exe2);
    let manifest_bytes = manifest.clone().into_bytes();
    let signature = sign(&signing_k1(), manifest.as_bytes());
    let envelope = envelope_json(&signing_k1(), &signature).into_bytes();
    let mut overserved = package.clone();
    overserved.extend_from_slice(b"garbage beyond the declared size");
    let addr = serve(move |target, addr| {
        if target.starts_with("/github/releases") {
            let release = release_json(
                1,
                "v1.4.0",
                false,
                false,
                &[
                    (
                        ENVELOPE_ASSET_NAME,
                        &format!("http://{addr}/download/{ENVELOPE_ASSET_NAME}"),
                    ),
                    (
                        MANIFEST_ASSET_NAME,
                        &format!("http://{addr}/download/{MANIFEST_ASSET_NAME}"),
                    ),
                    (
                        "desktop-todo-widget-v1.4.0-windows-x64.zip",
                        &format!(
                            "http://{addr}/download/desktop-todo-widget-v1.4.0-windows-x64.zip"
                        ),
                    ),
                ],
            );
            return http_ok(format!("[{release}]").as_bytes());
        }
        if target.ends_with(ENVELOPE_ASSET_NAME) {
            return http_ok(&envelope);
        }
        if target.ends_with(MANIFEST_ASSET_NAME) {
            return http_ok(&manifest_bytes);
        }
        if target.ends_with(".zip") {
            return chunked_response(
                "HTTP/1.1 200 OK",
                &[
                    &overserved[..overserved.len() / 2],
                    &overserved[overserved.len() / 2..],
                ],
            );
        }
        not_found()
    });
    let fixture = fixture(addr);
    let (target, envelope_bytes, _) = match run(&fixture, ReleaseSource::GitHub) {
        DiscoveryOutcome::TargetFound {
            target,
            envelope_bytes,
            ..
        } => (target, envelope_bytes, None::<String>),
        other => panic!("expected TargetFound, got {other:?}"),
    };
    let root = temp_root("cap");
    let mut session = persist_trusted_target(
        &root,
        *target,
        &envelope_bytes,
        Some(&format!(
            "http://{addr}/download/desktop-todo-widget-v1.4.0-windows-x64.zip"
        )),
        "GitHub",
        &fixture.installed,
    )
    .expect("persist");
    match acquire_and_stage(&mut session, &fixture.client) {
        Err(AcquisitionError::SizeMismatch { .. }) => {}
        other => panic!("expected SizeMismatch, got {other:?}"),
    }
    assert_eq!(session.record.state, MilestoneState::TrustedTargetPersisted);
    assert!(!session.dir.join(PACKAGE_FILE).exists());
    assert!(!session.dir.join(STAGED_DIR).exists());
    let _ = std::fs::remove_dir_all(&root);
}

/// Archive-level failures are typed and package-local: the record state
/// never advances and no staging survives.
#[test]
fn archive_rule_violations_are_typed_and_fail_closed() {
    let exe1 = exe_payload(1);
    let exe2 = exe_payload(2);
    let readme = b"readme".to_vec();

    struct Case {
        label: &'static str,
        package: Vec<u8>,
        expect: fn(&AcquisitionError) -> bool,
    }
    let cases = vec![
        Case {
            label: "extra file",
            package: zip_bytes(
                &[
                    ("desktop-todo-widget.exe", exe1.clone()),
                    ("desktop-todo-maintenance.exe", exe2.clone()),
                    ("install.ps1", readme.clone()),
                    ("uninstall.ps1", readme.clone()),
                    ("README.md", readme.clone()),
                    ("README_ZH.md", readme.clone()),
                    ("LICENSE", readme.clone()),
                    ("LICENSE_ZH.md", readme.clone()),
                    ("THIRD_PARTY_NOTICES.md", readme.clone()),
                    ("extra.txt", readme.clone()),
                ],
                zip::CompressionMethod::Deflated,
            ),
            expect: |error| {
                matches!(
                    error,
                    AcquisitionError::Archive(ArchiveError::AllowlistMismatch { .. })
                )
            },
        },
        Case {
            label: "missing file",
            package: zip_bytes(
                &[
                    ("desktop-todo-widget.exe", exe1.clone()),
                    ("desktop-todo-maintenance.exe", exe2.clone()),
                    ("install.ps1", readme.clone()),
                    ("uninstall.ps1", readme.clone()),
                    ("README.md", readme.clone()),
                    ("README_ZH.md", readme.clone()),
                    ("LICENSE", readme.clone()),
                    ("LICENSE_ZH.md", readme.clone()),
                ],
                zip::CompressionMethod::Deflated,
            ),
            expect: |error| {
                matches!(
                    error,
                    AcquisitionError::Archive(ArchiveError::AllowlistMismatch { .. })
                )
            },
        },
        Case {
            label: "nested path",
            package: zip_bytes(
                &[
                    ("desktop-todo-widget.exe", exe1.clone()),
                    ("desktop-todo-maintenance.exe", exe2.clone()),
                    ("install.ps1", readme.clone()),
                    ("uninstall.ps1", readme.clone()),
                    ("README.md", readme.clone()),
                    ("README_ZH.md", readme.clone()),
                    ("LICENSE", readme.clone()),
                    ("LICENSE_ZH.md", readme.clone()),
                    ("nested/THIRD_PARTY_NOTICES.md", readme.clone()),
                ],
                zip::CompressionMethod::Deflated,
            ),
            expect: |error| {
                matches!(
                    error,
                    AcquisitionError::Archive(ArchiveError::AllowlistMismatch { .. })
                )
            },
        },
        Case {
            label: "parent traversal",
            package: zip_bytes(
                &[
                    ("desktop-todo-widget.exe", exe1.clone()),
                    ("desktop-todo-maintenance.exe", exe2.clone()),
                    ("install.ps1", readme.clone()),
                    ("uninstall.ps1", readme.clone()),
                    ("README.md", readme.clone()),
                    ("README_ZH.md", readme.clone()),
                    ("LICENSE", readme.clone()),
                    ("LICENSE_ZH.md", readme.clone()),
                    ("../evil.md", readme.clone()),
                ],
                zip::CompressionMethod::Deflated,
            ),
            expect: |error| {
                matches!(
                    error,
                    AcquisitionError::Archive(ArchiveError::PathUnsafe { .. })
                )
            },
        },
        Case {
            label: "absolute path",
            package: zip_bytes(
                &[
                    ("desktop-todo-widget.exe", exe1.clone()),
                    ("desktop-todo-maintenance.exe", exe2.clone()),
                    ("install.ps1", readme.clone()),
                    ("uninstall.ps1", readme.clone()),
                    ("README.md", readme.clone()),
                    ("README_ZH.md", readme.clone()),
                    ("LICENSE", readme.clone()),
                    ("LICENSE_ZH.md", readme.clone()),
                    ("/etc/evil.md", readme.clone()),
                ],
                zip::CompressionMethod::Deflated,
            ),
            expect: |error| {
                matches!(
                    error,
                    AcquisitionError::Archive(ArchiveError::PathUnsafe { .. })
                )
            },
        },
        Case {
            label: "windows drive path",
            package: zip_bytes(
                &[
                    ("desktop-todo-widget.exe", exe1.clone()),
                    ("desktop-todo-maintenance.exe", exe2.clone()),
                    ("install.ps1", readme.clone()),
                    ("uninstall.ps1", readme.clone()),
                    ("README.md", readme.clone()),
                    ("README_ZH.md", readme.clone()),
                    ("LICENSE", readme.clone()),
                    ("LICENSE_ZH.md", readme.clone()),
                    ("C:evil.md", readme.clone()),
                ],
                zip::CompressionMethod::Deflated,
            ),
            expect: |error| {
                matches!(
                    error,
                    AcquisitionError::Archive(ArchiveError::PathUnsafe { .. })
                )
            },
        },
        Case {
            label: "ads colon",
            package: zip_bytes(
                &[
                    ("desktop-todo-widget.exe", exe1.clone()),
                    ("desktop-todo-maintenance.exe", exe2.clone()),
                    ("install.ps1", readme.clone()),
                    ("uninstall.ps1", readme.clone()),
                    ("README.md", readme.clone()),
                    ("README_ZH.md", readme.clone()),
                    ("LICENSE", readme.clone()),
                    ("LICENSE_ZH.md", readme.clone()),
                    ("THIRD_PARTY_NOTICES.md:hidden", readme.clone()),
                ],
                zip::CompressionMethod::Deflated,
            ),
            expect: |error| {
                matches!(
                    error,
                    AcquisitionError::Archive(ArchiveError::PathUnsafe { .. })
                )
            },
        },
        Case {
            label: "case alias",
            package: zip_bytes(
                &[
                    ("desktop-todo-widget.exe", exe1.clone()),
                    ("desktop-todo-maintenance.exe", exe2.clone()),
                    ("install.ps1", readme.clone()),
                    ("uninstall.ps1", readme.clone()),
                    ("README.md", readme.clone()),
                    ("README_ZH.md", readme.clone()),
                    ("LICENSE", readme.clone()),
                    ("LICENSE_ZH.MD", readme.clone()),
                    ("THIRD_PARTY_NOTICES.md", readme.clone()),
                ],
                zip::CompressionMethod::Deflated,
            ),
            expect: |error| {
                matches!(
                    error,
                    AcquisitionError::Archive(ArchiveError::AllowlistMismatch { .. })
                )
            },
        },
        Case {
            // The writer refuses an exactly duplicated name, so the dup is
            // expressed as a case-variant: the validator's case-insensitive
            // seen-set rejects it before the allowlist comparison.
            label: "case-insensitive duplicate entry",
            package: zip_bytes(
                &[
                    ("desktop-todo-widget.exe", exe1.clone()),
                    ("desktop-todo-maintenance.exe", exe2.clone()),
                    ("install.ps1", readme.clone()),
                    ("uninstall.ps1", readme.clone()),
                    ("README.md", readme.clone()),
                    ("README_ZH.md", readme.clone()),
                    ("readme.md", readme.clone()),
                    ("LICENSE", readme.clone()),
                    ("LICENSE_ZH.md", readme.clone()),
                    ("THIRD_PARTY_NOTICES.md", readme.clone()),
                ],
                zip::CompressionMethod::Deflated,
            ),
            expect: |error| {
                matches!(
                    error,
                    AcquisitionError::Archive(ArchiveError::DuplicateEntry { .. })
                )
            },
        },
        Case {
            label: "trailing dot",
            package: zip_bytes(
                &[
                    ("desktop-todo-widget.exe", exe1.clone()),
                    ("desktop-todo-maintenance.exe", exe2.clone()),
                    ("install.ps1", readme.clone()),
                    ("uninstall.ps1", readme.clone()),
                    ("README.md", readme.clone()),
                    ("README_ZH.md", readme.clone()),
                    ("LICENSE", readme.clone()),
                    ("LICENSE_ZH.md", readme.clone()),
                    ("THIRD_PARTY_NOTICES.md.", readme.clone()),
                ],
                zip::CompressionMethod::Deflated,
            ),
            expect: |error| {
                matches!(
                    error,
                    AcquisitionError::Archive(ArchiveError::PathUnsafe { .. })
                )
            },
        },
        Case {
            label: "compression bomb",
            package: {
                let bomb = vec![0u8; 20_000];
                zip_bytes(
                    &[
                        ("desktop-todo-widget.exe", exe1.clone()),
                        ("desktop-todo-maintenance.exe", exe2.clone()),
                        ("install.ps1", bomb),
                        ("uninstall.ps1", readme.clone()),
                        ("README.md", readme.clone()),
                        ("README_ZH.md", readme.clone()),
                        ("LICENSE", readme.clone()),
                        ("LICENSE_ZH.md", readme.clone()),
                        ("THIRD_PARTY_NOTICES.md", readme.clone()),
                    ],
                    zip::CompressionMethod::Deflated,
                )
            },
            expect: |error| {
                matches!(
                    error,
                    AcquisitionError::Archive(ArchiveError::CompressionRatioExceeded { .. })
                )
            },
        },
        Case {
            label: "corrupt zip",
            package: {
                let mut bytes = valid_package(&exe1, &exe2);
                let last = bytes.len() - 1;
                bytes[last] ^= 0xFF;
                bytes
            },
            expect: |error| {
                matches!(
                    error,
                    AcquisitionError::Archive(ArchiveError::Malformed { .. })
                )
            },
        },
    ];

    for case in cases {
        let manifest = pipeline_manifest("1.4.0", &case.package, &exe1, &exe2);
        let signature = sign(&signing_k1(), manifest.as_bytes());
        let envelope = envelope_json(&signing_k1(), &signature).into_bytes();
        let package = case.package.clone();
        let addr = serve(move |target, addr| {
            if target.starts_with("/github/releases") {
                let release = release_json(
                    1,
                    "v1.4.0",
                    false,
                    false,
                    &[
                        (
                            ENVELOPE_ASSET_NAME,
                            &format!("http://{addr}/download/{ENVELOPE_ASSET_NAME}"),
                        ),
                        (
                            MANIFEST_ASSET_NAME,
                            &format!("http://{addr}/download/{MANIFEST_ASSET_NAME}"),
                        ),
                        (
                            "desktop-todo-widget-v1.4.0-windows-x64.zip",
                            &format!(
                                "http://{addr}/download/desktop-todo-widget-v1.4.0-windows-x64.zip"
                            ),
                        ),
                    ],
                );
                return http_ok(format!("[{release}]").as_bytes());
            }
            if target.ends_with(ENVELOPE_ASSET_NAME) {
                return http_ok(&envelope);
            }
            if target.ends_with(MANIFEST_ASSET_NAME) {
                return http_ok(manifest.as_bytes());
            }
            if target.ends_with(".zip") {
                return http_ok(&package);
            }
            not_found()
        });
        let fixture = fixture(addr);
        let (target, envelope_bytes, _) = match run(&fixture, ReleaseSource::GitHub) {
            DiscoveryOutcome::TargetFound {
                target,
                envelope_bytes,
                ..
            } => (target, envelope_bytes, None::<String>),
            other => panic!("{}: expected TargetFound, got {other:?}", case.label),
        };
        let root = temp_root(case.label);
        let mut session = persist_trusted_target(
            &root,
            *target,
            &envelope_bytes,
            Some(&format!(
                "http://{addr}/download/desktop-todo-widget-v1.4.0-windows-x64.zip"
            )),
            "GitHub",
            &fixture.installed,
        )
        .unwrap_or_else(|e| panic!("{}: persist failed: {e}", case.label));
        let error = acquire_and_stage(&mut session, &fixture.client).expect_err(case.label);
        assert!(
            (case.expect)(&error),
            "{}: unexpected error {error:?}",
            case.label
        );
        assert_eq!(session.record.state, MilestoneState::TrustedTargetPersisted);
        assert!(!session.dir.join(STAGED_DIR).exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// The two managed executables are cross-checked against the signed
/// installFiles facts after extraction: package hash correctness alone
/// never substitutes for per-file verification.
#[test]
fn managed_executable_hash_mismatch_is_typed() {
    let exe1 = exe_payload(1);
    let exe2 = exe_payload(2);
    // The manifest declares a WRONG hash for the helper EXE; the package
    // itself contains the real bytes and hashes correctly at package level.
    let readme = b"readme".to_vec();
    let package = zip_bytes(
        &[
            ("desktop-todo-widget.exe", exe1.clone()),
            ("desktop-todo-maintenance.exe", exe2.clone()),
            ("install.ps1", readme.clone()),
            ("uninstall.ps1", readme.clone()),
            ("README.md", readme.clone()),
            ("README_ZH.md", readme.clone()),
            ("LICENSE", readme.clone()),
            ("LICENSE_ZH.md", readme.clone()),
            ("THIRD_PARTY_NOTICES.md", readme),
        ],
        zip::CompressionMethod::Deflated,
    );
    let wrong_helper_hash = "b".repeat(64);
    let manifest = {
        let psha = sha_of(&package);
        format!(
            concat!(
                r#"{{"schemaVersion":1,"appId":"net.alanfloyd.desktop","channel":"stable","version":"1.4.0","#,
                r#""publishedAt":"2026-10-01T00:00:00Z","notes":"","updaterProtocol":1,"#,
                r#""assets":{{"windows-x64":{{"filename":"desktop-todo-widget-v1.4.0-windows-x64.zip","size":{pkg},"sha256":"{psha}","installFiles":["#,
                r#"{{"identity":"mainExecutable","filename":"desktop-todo-widget.exe","size":{s1},"sha256":"{h1}"}},"#,
                r#"{{"identity":"maintenanceHelper","filename":"desktop-todo-maintenance.exe","size":{s2},"sha256":"{h2}"}}]}}}}}}"#
            ),
            pkg = package.len(),
            psha = &psha,
            s1 = exe1.len(),
            h1 = sha_of(&exe1),
            s2 = exe2.len(),
            h2 = &wrong_helper_hash,
        )
    };
    let signature = sign(&signing_k1(), manifest.as_bytes());
    let envelope = envelope_json(&signing_k1(), &signature).into_bytes();
    let addr = serve(move |target, addr| {
        if target.starts_with("/github/releases") {
            let release = release_json(
                1,
                "v1.4.0",
                false,
                false,
                &[
                    (
                        ENVELOPE_ASSET_NAME,
                        &format!("http://{addr}/download/{ENVELOPE_ASSET_NAME}"),
                    ),
                    (
                        MANIFEST_ASSET_NAME,
                        &format!("http://{addr}/download/{MANIFEST_ASSET_NAME}"),
                    ),
                    (
                        "desktop-todo-widget-v1.4.0-windows-x64.zip",
                        &format!(
                            "http://{addr}/download/desktop-todo-widget-v1.4.0-windows-x64.zip"
                        ),
                    ),
                ],
            );
            return http_ok(format!("[{release}]").as_bytes());
        }
        if target.ends_with(ENVELOPE_ASSET_NAME) {
            return http_ok(&envelope);
        }
        if target.ends_with(MANIFEST_ASSET_NAME) {
            return http_ok(manifest.as_bytes());
        }
        if target.ends_with(".zip") {
            return http_ok(&package);
        }
        not_found()
    });
    let fixture = fixture(addr);
    let (target, envelope_bytes, _) = match run(&fixture, ReleaseSource::GitHub) {
        DiscoveryOutcome::TargetFound {
            target,
            envelope_bytes,
            ..
        } => (target, envelope_bytes, None::<String>),
        other => panic!("expected TargetFound, got {other:?}"),
    };
    let root = temp_root("exe-hash");
    let mut session = persist_trusted_target(
        &root,
        *target,
        &envelope_bytes,
        Some(&format!(
            "http://{addr}/download/desktop-todo-widget-v1.4.0-windows-x64.zip"
        )),
        "GitHub",
        &fixture.installed,
    )
    .expect("persist");
    match acquire_and_stage(&mut session, &fixture.client) {
        Err(AcquisitionError::Archive(ArchiveError::ExtractedHashMismatch { file, .. })) => {
            assert_eq!(file, "desktop-todo-maintenance.exe");
        }
        other => panic!("expected ExtractedHashMismatch, got {other:?}"),
    }
    // Main EXE was staged and verified before the helper check failed; the
    // record state must still not claim readiness.
    assert_eq!(session.record.state, MilestoneState::TrustedTargetPersisted);
    let _ = std::fs::remove_dir_all(&root);
}

// ------------------------------------------------------- restart model A–F

/// A: a persisted trusted target survives restart with the same binding.
#[test]
fn restart_a_persisted_target_recovers_identically() {
    let exe1 = exe_payload(1);
    let exe2 = exe_payload(2);
    let package = valid_package(&exe1, &exe2);
    let manifest = pipeline_manifest("1.4.0", &package, &exe1, &exe2);
    let signature = sign(&signing_k1(), manifest.as_bytes());
    let envelope = envelope_json(&signing_k1(), &signature).into_bytes();
    let trust = TrustStore::from_raw_keys(&[signing_k1().verifying_key().to_bytes()]).unwrap();
    let target_manifest =
        desktop_todo_update_core::verify_and_parse(&trust, &envelope, manifest.as_bytes())
            .expect("fixture target");
    let root = temp_root("restart-a");
    let session = persist_trusted_target(
        &root,
        target_manifest,
        &envelope,
        Some("https://github.com/x/pkg.zip"),
        "GitHub",
        &Version::parse("1.2.0").unwrap(),
    )
    .expect("persist");
    let id = session.id.clone();

    let recovered =
        recover_session(&root, &id, &test_trust_store(), &installed_anchor()).expect("recover");
    assert_eq!(recovered.record, session.record);
    assert_eq!(
        recovered.record.manifest_sha256,
        session.record.manifest_sha256
    );
    assert_eq!(recovered.record.target_version, "1.4.0");
    let _ = std::fs::remove_dir_all(&root);
}

/// B: a half-written or tampered durable record can never read as trusted.
#[test]
fn restart_b_broken_record_fails_closed() {
    let root = temp_root("restart-b");
    let dir = root.join("sessions").join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&dir).unwrap();
    // Truncated JSON (a crash mid-write leaves no such file at all thanks
    // to atomic_write; a corrupted file must fail closed regardless).
    std::fs::write(dir.join(RECORD_FILE), b"{\"schemaVersion\":1,\"sta").unwrap();
    let id = dir.file_name().unwrap().to_string_lossy().to_string();
    match recover_session(&root, &id, &test_trust_store(), &installed_anchor()) {
        Err(crate::updater::acquisition::PersistError::Malformed { .. }) => {}
        other => panic!("expected Malformed, got {other:?}"),
    }
    // Unknown-field record fails closed.
    std::fs::write(
        dir.join(RECORD_FILE),
        br#"{"schemaVersion":1,"state":"trustedTargetPersisted","extra":1}"#,
    )
    .unwrap();
    match recover_session(&root, &id, &test_trust_store(), &installed_anchor()) {
        Err(crate::updater::acquisition::PersistError::Malformed { .. }) => {}
        other => panic!("expected Malformed for unknown field, got {other:?}"),
    }
    // Unsupported schema fails closed.
    std::fs::write(
        dir.join(RECORD_FILE),
        br#"{"schemaVersion":2,"state":"trustedTargetPersisted","targetVersion":"1.4.0","manifestSha256":"a","envelopeSha256":"b","installedSourceVersion":"1.2.0","package":{"filename":"p.zip","size":1,"sha256":"c"},"installFiles":[],"discoveredVia":"GitHub","packageUrl":"","createdAt":"2026-10-01T00:00:00Z"}"#,
    )
    .unwrap();
    match recover_session(&root, &id, &test_trust_store(), &installed_anchor()) {
        Err(crate::updater::acquisition::PersistError::UnsupportedSchema { .. }) => {}
        other => panic!("expected UnsupportedSchema, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// C: a partial package download is not a verified state — it is ignored,
/// overwritten by the next run, and the record state never claims readiness.
#[test]
fn restart_c_partial_download_is_never_ready() {
    let exe1 = exe_payload(1);
    let exe2 = exe_payload(2);
    let package = valid_package(&exe1, &exe2);
    let manifest = pipeline_manifest("1.4.0", &package, &exe1, &exe2);
    let signature = sign(&signing_k1(), manifest.as_bytes());
    let envelope = envelope_json(&signing_k1(), &signature).into_bytes();
    let addr = serve(move |target, addr| {
        if target.starts_with("/github/releases") {
            let release = release_json(
                1,
                "v1.4.0",
                false,
                false,
                &[
                    (
                        ENVELOPE_ASSET_NAME,
                        &format!("http://{addr}/download/{ENVELOPE_ASSET_NAME}"),
                    ),
                    (
                        MANIFEST_ASSET_NAME,
                        &format!("http://{addr}/download/{MANIFEST_ASSET_NAME}"),
                    ),
                    (
                        "desktop-todo-widget-v1.4.0-windows-x64.zip",
                        &format!(
                            "http://{addr}/download/desktop-todo-widget-v1.4.0-windows-x64.zip"
                        ),
                    ),
                ],
            );
            return http_ok(format!("[{release}]").as_bytes());
        }
        if target.ends_with(ENVELOPE_ASSET_NAME) {
            return http_ok(&envelope);
        }
        if target.ends_with(MANIFEST_ASSET_NAME) {
            return http_ok(manifest.as_bytes());
        }
        if target.ends_with(".zip") {
            return http_ok(&package);
        }
        not_found()
    });
    let fixture = fixture(addr);
    let (target, envelope_bytes, _) = match run(&fixture, ReleaseSource::GitHub) {
        DiscoveryOutcome::TargetFound {
            target,
            envelope_bytes,
            ..
        } => (target, envelope_bytes, None::<String>),
        other => panic!("expected TargetFound, got {other:?}"),
    };
    let root = temp_root("restart-c");
    let session = persist_trusted_target(
        &root,
        *target,
        &envelope_bytes,
        Some(&format!(
            "http://{addr}/download/desktop-todo-widget-v1.4.0-windows-x64.zip"
        )),
        "GitHub",
        &fixture.installed,
    )
    .expect("persist");
    // Simulate a crash mid-download: a partial temp file and no package.
    std::fs::write(session.dir.join(PACKAGE_DOWNLOADING_FILE), b"partial").unwrap();
    assert!(!session.dir.join(PACKAGE_FILE).exists());

    let recovered = recover_session(&root, &session.id, &test_trust_store(), &installed_anchor())
        .expect("recover");
    assert_eq!(
        recovered.record.state,
        MilestoneState::TrustedTargetPersisted
    );
    let mut recovered = recovered;
    acquire_and_stage(&mut recovered, &fixture.client).expect("re-acquire");
    assert_eq!(recovered.record.state, MilestoneState::PackageStaged);
    assert!(!recovered.dir.join(PACKAGE_DOWNLOADING_FILE).exists());
    let _ = std::fs::remove_dir_all(&root);
}

/// D: a crash during extraction (after the package hash passed) leaves the
/// typed state at TrustedTargetPersisted — never Ready — and the next run
/// re-stages cleanly over the partial extraction.
#[test]
fn restart_d_partial_extraction_is_never_ready() {
    let exe1 = exe_payload(1);
    let exe2 = exe_payload(2);
    let package = valid_package(&exe1, &exe2);
    let manifest = pipeline_manifest("1.4.0", &package, &exe1, &exe2);
    let manifest_bytes = manifest.clone().into_bytes();
    let package_for_writes = package.clone();
    let signature = sign(&signing_k1(), manifest.as_bytes());
    let envelope = envelope_json(&signing_k1(), &signature).into_bytes();
    let addr = serve(move |target, addr| {
        if target.starts_with("/github/releases") {
            let release = release_json(
                1,
                "v1.4.0",
                false,
                false,
                &[
                    (
                        ENVELOPE_ASSET_NAME,
                        &format!("http://{addr}/download/{ENVELOPE_ASSET_NAME}"),
                    ),
                    (
                        MANIFEST_ASSET_NAME,
                        &format!("http://{addr}/download/{MANIFEST_ASSET_NAME}"),
                    ),
                    (
                        "desktop-todo-widget-v1.4.0-windows-x64.zip",
                        &format!(
                            "http://{addr}/download/desktop-todo-widget-v1.4.0-windows-x64.zip"
                        ),
                    ),
                ],
            );
            return http_ok(format!("[{release}]").as_bytes());
        }
        if target.ends_with(ENVELOPE_ASSET_NAME) {
            return http_ok(&envelope);
        }
        if target.ends_with(MANIFEST_ASSET_NAME) {
            return http_ok(&manifest_bytes);
        }
        if target.ends_with(".zip") {
            return http_ok(&package);
        }
        not_found()
    });
    let fixture = fixture(addr);
    let (target, envelope_bytes, _) = match run(&fixture, ReleaseSource::GitHub) {
        DiscoveryOutcome::TargetFound {
            target,
            envelope_bytes,
            ..
        } => (target, envelope_bytes, None::<String>),
        other => panic!("expected TargetFound, got {other:?}"),
    };
    let root = temp_root("restart-d");
    let session = persist_trusted_target(
        &root,
        *target,
        &envelope_bytes,
        Some(&format!(
            "http://{addr}/download/desktop-todo-widget-v1.4.0-windows-x64.zip"
        )),
        "GitHub",
        &fixture.installed,
    )
    .expect("persist");
    // Simulate a crash after download+hash but mid-extraction: package
    // published, one partial staged temp file, state never advanced.
    std::fs::write(session.dir.join(PACKAGE_FILE), &package_for_writes).unwrap();
    let staged = session.dir.join(STAGED_DIR);
    std::fs::create_dir_all(&staged).unwrap();
    std::fs::write(staged.join("desktop-todo-widget.exe.extracting"), b"half").unwrap();
    let recovered = recover_session(&root, &session.id, &test_trust_store(), &installed_anchor())
        .expect("recover");
    assert_eq!(
        recovered.record.state,
        MilestoneState::TrustedTargetPersisted,
        "partial extraction must never read as Ready"
    );
    let mut recovered = recovered;
    acquire_and_stage(&mut recovered, &fixture.client).expect("re-stage");
    assert_eq!(recovered.record.state, MilestoneState::PackageStaged);
    assert!(!staged.join("desktop-todo-widget.exe.extracting").exists());
    let _ = std::fs::remove_dir_all(&root);
}

/// E: a fully staged session recovers as Ready and re-verification passes
/// idempotently; a tampered staged file fails closed.
#[test]
fn restart_e_staged_state_recovers_and_reverifies() {
    let exe1 = exe_payload(1);
    let exe2 = exe_payload(2);
    let package = valid_package(&exe1, &exe2);
    let manifest = pipeline_manifest("1.4.0", &package, &exe1, &exe2);
    let signature = sign(&signing_k1(), manifest.as_bytes());
    let envelope = envelope_json(&signing_k1(), &signature).into_bytes();
    let addr = serve(move |target, addr| {
        if target.starts_with("/github/releases") {
            let release = release_json(
                1,
                "v1.4.0",
                false,
                false,
                &[
                    (
                        ENVELOPE_ASSET_NAME,
                        &format!("http://{addr}/download/{ENVELOPE_ASSET_NAME}"),
                    ),
                    (
                        MANIFEST_ASSET_NAME,
                        &format!("http://{addr}/download/{MANIFEST_ASSET_NAME}"),
                    ),
                    (
                        "desktop-todo-widget-v1.4.0-windows-x64.zip",
                        &format!(
                            "http://{addr}/download/desktop-todo-widget-v1.4.0-windows-x64.zip"
                        ),
                    ),
                ],
            );
            return http_ok(format!("[{release}]").as_bytes());
        }
        if target.ends_with(ENVELOPE_ASSET_NAME) {
            return http_ok(&envelope);
        }
        if target.ends_with(MANIFEST_ASSET_NAME) {
            return http_ok(manifest.as_bytes());
        }
        if target.ends_with(".zip") {
            return http_ok(&package);
        }
        not_found()
    });
    let fixture = fixture(addr);
    let (target, envelope_bytes, _) = match run(&fixture, ReleaseSource::GitHub) {
        DiscoveryOutcome::TargetFound {
            target,
            envelope_bytes,
            ..
        } => (target, envelope_bytes, None::<String>),
        other => panic!("expected TargetFound, got {other:?}"),
    };
    let root = temp_root("restart-e");
    let mut session = persist_trusted_target(
        &root,
        *target,
        &envelope_bytes,
        Some(&format!(
            "http://{addr}/download/desktop-todo-widget-v1.4.0-windows-x64.zip"
        )),
        "GitHub",
        &fixture.installed,
    )
    .expect("persist");
    acquire_and_stage(&mut session, &fixture.client).expect("stage");

    let mut recovered =
        recover_session(&root, &session.id, &test_trust_store(), &installed_anchor())
            .expect("recover");
    assert_eq!(recovered.record.state, MilestoneState::PackageStaged);
    // Idempotent: staged files still verify without any network.
    acquire_and_stage(&mut recovered, &fixture.client).expect("reverify");
    // Tampering with a staged file fails closed on the next verification.
    let staged = recovered.dir.join(STAGED_DIR);
    std::fs::write(staged.join("desktop-todo-widget.exe"), vec![0xFF; 1000]).unwrap();
    match acquire_and_stage(&mut recovered, &fixture.client) {
        Err(AcquisitionError::Archive(ArchiveError::ExtractedHashMismatch { .. })) => {}
        other => panic!("expected ExtractedHashMismatch, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// F: staged state is bound to its own session and target — swapping the
/// persisted manifest bytes of another session breaks the digest binding
/// and fails closed instead of rebinding.
#[test]
fn restart_f_staged_state_never_binds_a_new_target() {
    let exe1 = exe_payload(1);
    let exe2 = exe_payload(2);
    let package = valid_package(&exe1, &exe2);

    // Session A: a real staged target 1.4.0.
    let manifest_a = pipeline_manifest("1.4.0", &package, &exe1, &exe2);
    let signature_a = sign(&signing_k1(), manifest_a.as_bytes());
    let envelope_a = envelope_json(&signing_k1(), &signature_a).into_bytes();
    let manifest_bytes = manifest_a.clone().into_bytes();
    let package_fixture = package.clone();
    let envelope_fixture = envelope_a.clone();
    let addr = serve(move |target, addr| {
        if target.starts_with("/github/releases") {
            let release = release_json(
                1,
                "v1.4.0",
                false,
                false,
                &[
                    (
                        ENVELOPE_ASSET_NAME,
                        &format!("http://{addr}/download/{ENVELOPE_ASSET_NAME}"),
                    ),
                    (
                        MANIFEST_ASSET_NAME,
                        &format!("http://{addr}/download/{MANIFEST_ASSET_NAME}"),
                    ),
                    (
                        "desktop-todo-widget-v1.4.0-windows-x64.zip",
                        &format!(
                            "http://{addr}/download/desktop-todo-widget-v1.4.0-windows-x64.zip"
                        ),
                    ),
                ],
            );
            return http_ok(format!("[{release}]").as_bytes());
        }
        if target.ends_with(ENVELOPE_ASSET_NAME) {
            return http_ok(&envelope_fixture);
        }
        if target.ends_with(MANIFEST_ASSET_NAME) {
            return http_ok(&manifest_bytes);
        }
        if target.ends_with(".zip") {
            return http_ok(&package_fixture);
        }
        not_found()
    });
    let fixture = fixture(addr);
    let (target, envelope_bytes, _) = match run(&fixture, ReleaseSource::GitHub) {
        DiscoveryOutcome::TargetFound {
            target,
            envelope_bytes,
            ..
        } => (target, envelope_bytes, None::<String>),
        other => panic!("expected TargetFound, got {other:?}"),
    };
    let root = temp_root("restart-f");
    let mut session = persist_trusted_target(
        &root,
        *target,
        &envelope_bytes,
        Some(&format!(
            "http://{addr}/download/desktop-todo-widget-v1.4.0-windows-x64.zip"
        )),
        "GitHub",
        &fixture.installed,
    )
    .expect("persist");
    acquire_and_stage(&mut session, &fixture.client).expect("stage");
    let id_a = session.id.clone();

    // Swap session A's persisted manifest bytes for a DIFFERENT validly
    // signed target (1.3.0): the record's digest binding fails closed first,
    // and even a fully re-signed swap would disagree field-by-field with the
    // record. A staged state never rebinds to a new target.
    let other_manifest = manifest_text("1.3.0", 1);
    let other_signature = sign(&signing_k1(), other_manifest.as_bytes());
    let other_envelope = envelope_json(&signing_k1(), &other_signature).into_bytes();
    std::fs::write(session.dir.join(MANIFEST_FILE), other_manifest.as_bytes()).unwrap();
    std::fs::write(session.dir.join(ENVELOPE_FILE), &other_envelope).unwrap();
    match recover_session(&root, &id_a, &test_trust_store(), &installed_anchor()) {
        Err(crate::updater::acquisition::PersistError::DigestMismatch { file }) => {
            assert_eq!(file, MANIFEST_FILE);
        }
        other => panic!("expected DigestMismatch, got {other:?}"),
    }

    // Even when the swap is accompanied by a fully updated record (digests
    // recomputed over the swapped bytes — an attacker-or-bug scenario that
    // also survives the digest layer), the re-verified target disagrees
    // field-by-field with the record's staged facts and the source baseline
    // anchor is re-checked. Recovery still fails closed.
    let mut swapped_record: crate::updater::acquisition::TrustedTargetRecord =
        serde_json::from_slice(&std::fs::read(session.dir.join(RECORD_FILE)).unwrap()).unwrap();
    let fresh = recover_helper_target(other_manifest.as_bytes(), &other_envelope);
    swapped_record.manifest_sha256 = fresh.manifest_sha256_hex().to_string();
    swapped_record.envelope_sha256 = desktop_todo_update_core::sha256_hex(&other_envelope);
    swapped_record.target_version = "1.3.0".to_string();
    swapped_record.package.filename = "desktop-todo-widget-v1.3.0-windows-x64.zip".to_string();
    std::fs::write(
        session.dir.join(RECORD_FILE),
        serde_json::to_vec(&swapped_record).unwrap(),
    )
    .unwrap();
    match recover_session(&root, &id_a, &test_trust_store(), &installed_anchor()) {
        Err(crate::updater::acquisition::PersistError::RecordTampered { field }) => {
            assert_eq!(field, "package");
        }
        other => panic!("expected RecordTampered, got {other:?}"),
    }

    // The source-version anchor is checked against the CURRENT actual
    // installed version: the same intact session recovers under 1.2.0 but
    // fails closed when the runtime has changed (rollback/reinstall).
    std::fs::write(session.dir.join(MANIFEST_FILE), manifest_a.as_bytes()).unwrap();
    std::fs::write(session.dir.join(ENVELOPE_FILE), &envelope_bytes).unwrap();
    let mut swapped_record = serde_json::from_slice::<
        crate::updater::acquisition::TrustedTargetRecord,
    >(&std::fs::read(session.dir.join(RECORD_FILE)).unwrap())
    .unwrap();
    swapped_record.target_version = "1.4.0".to_string();
    let fresh = recover_helper_target(manifest_a.as_bytes(), &envelope_bytes);
    swapped_record.manifest_sha256 = fresh.manifest_sha256_hex().to_string();
    swapped_record.envelope_sha256 = desktop_todo_update_core::sha256_hex(&envelope_bytes);
    swapped_record.package.filename = "desktop-todo-widget-v1.4.0-windows-x64.zip".to_string();
    swapped_record.package.size = package.len() as u64;
    swapped_record.package.sha256 = sha_of(&package);
    swapped_record.install_files = fresh
        .manifest()
        .asset()
        .install_files()
        .iter()
        .map(|entry| crate::updater::acquisition::InstallFileFact {
            identity: entry.identity().to_string(),
            filename: entry.filename().to_string(),
            size: entry.size(),
            sha256: entry.sha256_hex().to_string(),
        })
        .collect();
    std::fs::write(
        session.dir.join(RECORD_FILE),
        serde_json::to_vec(&swapped_record).unwrap(),
    )
    .unwrap();
    assert!(recover_session(&root, &id_a, &test_trust_store(), &installed_anchor()).is_ok());
    match recover_session(
        &root,
        &id_a,
        &test_trust_store(),
        &Version::parse("1.3.0").unwrap(),
    ) {
        Err(crate::updater::acquisition::PersistError::SourceVersionChanged {
            recorded,
            current,
        }) => {
            assert_eq!(recorded, "1.2.0");
            assert_eq!(current, "1.3.0");
        }
        other => panic!("expected SourceVersionChanged, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// Build a VerifiedTarget from signed fixture bytes (test convenience for
/// hand-constructed session states).
fn recover_helper_target(manifest: &[u8], envelope: &[u8]) -> VerifiedTarget {
    desktop_todo_update_core::verify_and_parse(&test_trust_store(), envelope, manifest).unwrap()
}

/// Whole-scan budget (Phase 2B-B debt closure): an expired budget stops the
/// scan from taking new candidates, is recorded but never classified as a
/// candidate verdict or provider-layer failure, never drops an accepted
/// target, and does not trigger Auto fallback.
#[test]
fn scan_budget_stops_the_scan_without_provider_semantics() {
    let gitee_hits = Arc::new(AtomicUsize::new(0));
    let gitee_hits_clone = gitee_hits.clone();
    let addr = serve(move |target, addr| {
        if target.starts_with("/github/releases") {
            // Three eligible-looking candidates; the budget must stop the
            // scan before they are all taken.
            let one = release_with_assets(addr, 1, "one", "1.3.0");
            let two = release_with_assets(addr, 2, "two", "1.4.0");
            let three = release_with_assets(addr, 3, "three", "1.5.0");
            return http_ok(format!("[{one},{two},{three}]").as_bytes());
        }
        if target.starts_with("/gitee/") {
            gitee_hits_clone.fetch_add(1, Ordering::SeqCst);
        }
        serve_candidate_bytes(target, "one", "1.3.0", 1, &signing_k1())
            .or_else(|| serve_candidate_bytes(target, "two", "1.4.0", 1, &signing_k1()))
            .or_else(|| serve_candidate_bytes(target, "three", "1.5.0", 1, &signing_k1()))
            .unwrap_or_else(not_found)
    });
    let mut fixture = fixture(addr);
    fixture.config.scan_budget = Some(Duration::ZERO);
    let ctx = DiscoveryContext {
        config: &fixture.config,
        scan_started: Instant::now(),
        client: &fixture.client,
        endpoints: &fixture.endpoints,
        trust: &fixture.trust,
        installed_version: &fixture.installed,
        hop_policy: &fixture.hop,
    };
    match discover(ReleaseSource::Auto, &ctx) {
        DiscoveryOutcome::NoEligibleCandidate { scans } => {
            assert!(scans[0].budget_exhausted, "budget stop must be recorded");
        }
        other => panic!("expected NoEligibleCandidate, got {other:?}"),
    }
    assert_eq!(
        gitee_hits.load(Ordering::SeqCst),
        0,
        "budget exhaustion is client-side, not a provider-layer failure: no fallback"
    );

    // A generous budget never discards an accepted target (same run shape).
    fixture.config.scan_budget = Some(Duration::from_secs(60));
    let ctx = DiscoveryContext {
        config: &fixture.config,
        scan_started: Instant::now(),
        client: &fixture.client,
        endpoints: &fixture.endpoints,
        trust: &fixture.trust,
        installed_version: &fixture.installed,
        hop_policy: &fixture.hop,
    };
    assert_target(discover(ReleaseSource::GitHub, &ctx), "1.3.0");
}

/// The persisted raw manifest digest is the handoff binding: any single-byte
/// change to the raw bytes yields a different digest, so a different target
/// can never inherit another target's binding.
#[test]
fn expected_manifest_digest_changes_with_any_raw_byte() {
    let base = manifest_text("1.4.0", 1);
    let digest = desktop_todo_update_core::sha256_hex(base.as_bytes());
    let mut mutated = base.clone().into_bytes();
    mutated.insert(mutated.len() - 1, b' ');
    let mutated_digest = desktop_todo_update_core::sha256_hex(&mutated);
    assert_ne!(digest, mutated_digest);
}

// ===================================== durable boundary review tests (2C-A)

/// Shared setup for the durable-boundary tests: a real staged session built
/// through the full pipeline.
fn staged_session(
    tag: &str,
) -> (
    crate::updater::acquisition::UpdateSession,
    PathBuf,
    Vec<u8>,
    reqwest::blocking::Client,
) {
    let exe1 = exe_payload(1);
    let exe2 = exe_payload(2);
    let package = valid_package(&exe1, &exe2);
    let manifest = pipeline_manifest("1.4.0", &package, &exe1, &exe2);
    let signature = sign(&signing_k1(), manifest.as_bytes());
    let envelope = envelope_json(&signing_k1(), &signature).into_bytes();
    let package_fixture = package.clone();
    let addr = serve(move |target, addr| {
        if target.starts_with("/github/releases") {
            let release = release_json(
                1,
                "v1.4.0",
                false,
                false,
                &[
                    (
                        ENVELOPE_ASSET_NAME,
                        &format!("http://{addr}/download/{ENVELOPE_ASSET_NAME}"),
                    ),
                    (
                        MANIFEST_ASSET_NAME,
                        &format!("http://{addr}/download/{MANIFEST_ASSET_NAME}"),
                    ),
                    (
                        "desktop-todo-widget-v1.4.0-windows-x64.zip",
                        &format!(
                            "http://{addr}/download/desktop-todo-widget-v1.4.0-windows-x64.zip"
                        ),
                    ),
                ],
            );
            return http_ok(format!("[{release}]").as_bytes());
        }
        if target.ends_with(ENVELOPE_ASSET_NAME) {
            return http_ok(&envelope);
        }
        if target.ends_with(MANIFEST_ASSET_NAME) {
            return http_ok(manifest.as_bytes());
        }
        if target.ends_with(".zip") {
            return http_ok(&package_fixture);
        }
        not_found()
    });
    let fixture = fixture(addr);
    let (target, envelope_bytes, _) = match run(&fixture, ReleaseSource::GitHub) {
        DiscoveryOutcome::TargetFound {
            target,
            envelope_bytes,
            ..
        } => (target, envelope_bytes, None::<String>),
        other => panic!("expected TargetFound, got {other:?}"),
    };
    let root = temp_root(tag);
    let mut session = persist_trusted_target(
        &root,
        *target,
        &envelope_bytes,
        Some(&format!(
            "http://{addr}/download/desktop-todo-widget-v1.4.0-windows-x64.zip"
        )),
        "GitHub",
        &fixture.installed,
    )
    .expect("persist");
    acquire_and_stage(&mut session, &fixture.client).expect("stage");
    (session, root, package, fixture.client)
}

fn read_record(
    session: &crate::updater::acquisition::UpdateSession,
) -> crate::updater::acquisition::TrustedTargetRecord {
    serde_json::from_slice(&std::fs::read(session.dir.join(RECORD_FILE)).unwrap()).unwrap()
}

fn write_record(
    session: &crate::updater::acquisition::UpdateSession,
    record: &crate::updater::acquisition::TrustedTargetRecord,
) {
    std::fs::write(
        session.dir.join(RECORD_FILE),
        serde_json::to_vec(record).unwrap(),
    )
    .unwrap();
}

/// T1: only the record's package SHA-256 is tampered with (raw bytes and
/// digests untouched) — recovery fails closed; the re-verified target, not
/// the record, is the authority.
#[test]
fn t1_tampered_record_package_sha_fails_closed() {
    let (mut session, root, package, client) = staged_session("t1");
    let mut record = read_record(&session);
    record.package.sha256 = "e".repeat(64);
    write_record(&session, &record);
    match recover_session(&root, &session.id, &test_trust_store(), &installed_anchor()) {
        Err(crate::updater::acquisition::PersistError::RecordTampered { field }) => {
            assert_eq!(field, "package");
        }
        other => panic!("expected RecordTampered, got {other:?}"),
    }
    assert_eq!(
        std::fs::read(session.dir.join(PACKAGE_FILE)).unwrap(),
        package
    );
    acquire_and_stage(&mut session, &client).expect("intact in-memory session still works");
    let _ = std::fs::remove_dir_all(&root);
}

/// T2: only the record's installFiles facts are tampered with — they can
/// never become authoritative.
#[test]
fn t2_tampered_record_install_files_fail_closed() {
    let (session, root, _package, _client) = staged_session("t2");
    let mut record = read_record(&session);
    record.install_files[0].sha256 = "f".repeat(64);
    write_record(&session, &record);
    match recover_session(&root, &session.id, &test_trust_store(), &installed_anchor()) {
        Err(crate::updater::acquisition::PersistError::RecordTampered { field }) => {
            assert_eq!(field, "installFiles");
        }
        other => panic!("expected RecordTampered, got {other:?}"),
    }
    record.install_files[0].size = 9_999_999;
    write_record(&session, &record);
    match recover_session(&root, &session.id, &test_trust_store(), &installed_anchor()) {
        Err(crate::updater::acquisition::PersistError::RecordTampered { field }) => {
            assert_eq!(field, "installFiles");
        }
        other => panic!("expected RecordTampered for size, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// T3: one changed raw-manifest byte — even when the record's digest is
/// recomputed to match — fails the signature recovery layer.
#[test]
fn t3_manifest_byte_change_fails_signature_recovery() {
    let (session, root, _package, _client) = staged_session("t3");
    let mut tampered = std::fs::read(session.dir.join(MANIFEST_FILE)).unwrap();
    tampered.insert(tampered.len() - 1, b' ');
    std::fs::write(session.dir.join(MANIFEST_FILE), &tampered).unwrap();
    let mut record = read_record(&session);
    record.manifest_sha256 = desktop_todo_update_core::sha256_hex(&tampered);
    write_record(&session, &record);
    match recover_session(&root, &session.id, &test_trust_store(), &installed_anchor()) {
        Err(crate::updater::acquisition::PersistError::SignatureRecovery { detail }) => {
            assert!(detail.contains("BadSignature"), "{detail}");
        }
        other => panic!("expected SignatureRecovery, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// T4: one changed envelope byte (with the record digest recomputed) fails
/// the signature recovery layer — the envelope is verified input too.
#[test]
fn t4_envelope_byte_change_fails_signature_recovery() {
    let (session, root, _package, _client) = staged_session("t4");
    let mut tampered = std::fs::read(session.dir.join(ENVELOPE_FILE)).unwrap();
    // Non-whitespace trailing byte: JSON whitespace would leave the selected
    // signature and target semantically identical, but any byte change that
    // alters the envelope document must fail signature recovery.
    tampered.insert(tampered.len(), b'x');
    std::fs::write(session.dir.join(ENVELOPE_FILE), &tampered).unwrap();
    let mut record = read_record(&session);
    record.envelope_sha256 = desktop_todo_update_core::sha256_hex(&tampered);
    write_record(&session, &record);
    match recover_session(&root, &session.id, &test_trust_store(), &installed_anchor()) {
        Err(crate::updater::acquisition::PersistError::SignatureRecovery { detail }) => {
            assert!(detail.contains("EnvelopeMalformed"), "{detail}");
        }
        other => panic!("expected SignatureRecovery, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// The package locator selection authority is the signed manifest filename,
/// not any extension or naming grammar: a package asset named
/// `package.payload` whose bytes are a fully valid ZIP flows through the
/// whole pipeline.
#[test]
fn package_locator_is_the_signed_filename_not_an_extension() {
    let exe1 = exe_payload(1);
    let exe2 = exe_payload(2);
    let package = valid_package(&exe1, &exe2);
    let manifest = {
        let psha = sha_of(&package);
        format!(
            concat!(
                r#"{{"schemaVersion":1,"appId":"net.alanfloyd.desktop","channel":"stable","version":"1.4.0","#,
                r#""publishedAt":"2026-10-01T00:00:00Z","notes":"","updaterProtocol":1,"#,
                r#""assets":{{"windows-x64":{{"filename":"package.payload","size":{pkg},"sha256":"{psha}","installFiles":["#,
                r#"{{"identity":"mainExecutable","filename":"desktop-todo-widget.exe","size":{s1},"sha256":"{h1}"}},"#,
                r#"{{"identity":"maintenanceHelper","filename":"desktop-todo-maintenance.exe","size":{s2},"sha256":"{h2}"}}]}}}}}}"#
            ),
            pkg = package.len(),
            psha = &psha,
            s1 = exe1.len(),
            h1 = sha_of(&exe1),
            s2 = exe2.len(),
            h2 = sha_of(&exe2),
        )
    };
    let manifest_bytes = manifest.clone().into_bytes();
    let signature = sign(&signing_k1(), manifest.as_bytes());
    let envelope = envelope_json(&signing_k1(), &signature).into_bytes();
    let package_fixture = package.clone();
    let addr = serve(move |target, addr| {
        if target.starts_with("/github/releases") {
            let release = release_json(
                1,
                "v1.4.0",
                false,
                false,
                &[
                    (
                        ENVELOPE_ASSET_NAME,
                        &format!("http://{addr}/download/{ENVELOPE_ASSET_NAME}"),
                    ),
                    (
                        MANIFEST_ASSET_NAME,
                        &format!("http://{addr}/download/{MANIFEST_ASSET_NAME}"),
                    ),
                    (
                        "package.payload",
                        &format!("http://{addr}/download/package.payload"),
                    ),
                ],
            );
            return http_ok(format!("[{release}]").as_bytes());
        }
        if target.ends_with(ENVELOPE_ASSET_NAME) {
            return http_ok(&envelope);
        }
        if target.ends_with(MANIFEST_ASSET_NAME) {
            return http_ok(&manifest_bytes);
        }
        if target.ends_with("package.payload") {
            return http_ok(&package_fixture);
        }
        not_found()
    });
    let fixture = fixture(addr);
    let (target, envelope_bytes, package_url) = match run(&fixture, ReleaseSource::GitHub) {
        DiscoveryOutcome::TargetFound {
            target,
            envelope_bytes,
            package_url,
            ..
        } => (target, envelope_bytes, package_url),
        other => panic!("expected TargetFound, got {other:?}"),
    };
    let package_url = package_url.expect("the exact signed filename selects the package locator");
    assert!(package_url.ends_with("package.payload"));
    let root = temp_root("opaque-name");
    let mut session = persist_trusted_target(
        &root,
        *target,
        &envelope_bytes,
        Some(&package_url),
        "GitHub",
        &fixture.installed,
    )
    .expect("persist");
    acquire_and_stage(&mut session, &fixture.client).expect("acquire");
    assert_eq!(session.record.state, MilestoneState::PackageStaged);
    let _ = std::fs::remove_dir_all(&root);
}

/// Two provider assets with the same exact signed filename are an ambiguous
/// locator: the candidate is skipped locally and an older bridge is accepted
/// — never "take the first one".
#[test]
fn ambiguous_package_assets_skip_the_candidate() {
    let addr = serve(move |target, addr| {
        if target.starts_with("/github/releases") {
            let newest = release_json(
                9,
                "v1.6.0",
                false,
                false,
                &[
                    (
                        ENVELOPE_ASSET_NAME,
                        &format!("http://{addr}/download/newest/{ENVELOPE_ASSET_NAME}"),
                    ),
                    (
                        MANIFEST_ASSET_NAME,
                        &format!("http://{addr}/download/newest/{MANIFEST_ASSET_NAME}"),
                    ),
                    (
                        "desktop-todo-widget-v1.6.0-windows-x64.zip",
                        &format!(
                            "http://{addr}/download/newest/first-desktop-todo-widget-v1.6.0-windows-x64.zip"
                        ),
                    ),
                    (
                        "desktop-todo-widget-v1.6.0-windows-x64.zip",
                        &format!(
                            "http://{addr}/download/newest/second-desktop-todo-widget-v1.6.0-windows-x64.zip"
                        ),
                    ),
                ],
            );
            let bridge = release_with_assets(addr, 2, "bridge", "1.4.0");
            return http_ok(format!("[{newest},{bridge}]").as_bytes());
        }
        if target.contains("/download/newest/") {
            let (envelope, manifest_bytes) = signed_metadata(&signing_k1(), "1.6.0", 1);
            return if target.ends_with(ENVELOPE_ASSET_NAME) {
                http_ok(&envelope)
            } else {
                http_ok(&manifest_bytes)
            };
        }
        serve_candidate_bytes(target, "bridge", "1.4.0", 1, &signing_k1()).unwrap_or_else(not_found)
    });
    let fixture = fixture(addr);
    let target = assert_target(run(&fixture, ReleaseSource::GitHub), "1.4.0");
    assert_eq!(
        target.manifest().version().to_string(),
        "1.4.0",
        "the ambiguous newest candidate was skipped, not guessed"
    );
}

/// Hard whole-scan deadline: a 300ms budget with a 30s per-request timeout
/// cannot hang for 30s — every attempt's timeout, retry, and sleep is
/// bounded by the remaining budget, and the expiry is a client-side stop
/// (NoEligibleCandidate with the budget flag), never a provider failure and
/// never a trigger for Auto fallback.
#[test]
fn hard_deadline_bounds_hung_requests() {
    let gitee_hits = Arc::new(AtomicUsize::new(0));
    let gitee_hits_clone = gitee_hits.clone();
    let github_hits = Arc::new(AtomicUsize::new(0));
    let github_hits_clone = github_hits.clone();
    let addr = serve(move |target, _| {
        if target.starts_with("/github/releases") {
            github_hits_clone.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_secs(30));
            return http_ok(b"[]");
        }
        if target.starts_with("/gitee/") {
            gitee_hits_clone.fetch_add(1, Ordering::SeqCst);
        }
        not_found()
    });
    let mut fixture = fixture(addr);
    fixture.config.scan_budget = Some(Duration::from_millis(300));
    fixture.config.request_timeout = Duration::from_secs(30);
    fixture.client = discovery_client(
        &fixture.config,
        &combined_origin_allowlist(&fixture.endpoints),
    )
    .unwrap();
    let started = Instant::now();
    let ctx = DiscoveryContext {
        config: &fixture.config,
        scan_started: Instant::now(),
        client: &fixture.client,
        endpoints: &fixture.endpoints,
        trust: &fixture.trust,
        installed_version: &fixture.installed,
        hop_policy: &fixture.hop,
    };
    match discover(ReleaseSource::Auto, &ctx) {
        DiscoveryOutcome::NoEligibleCandidate { scans } => {
            assert!(scans[0].budget_exhausted);
        }
        other => panic!("expected NoEligibleCandidate, got {other:?}"),
    }
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the 30s per-request timeout must be clamped by the 300ms budget"
    );
    assert_eq!(
        gitee_hits.load(Ordering::SeqCst),
        0,
        "budget expiry is client-side: no Auto fallback"
    );
    assert!(github_hits.load(Ordering::SeqCst) >= 1);
}

/// A Retry-After longer than the remaining budget is not slept through.
#[test]
fn retry_after_beyond_the_budget_stops_instead_of_sleeping() {
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = hits.clone();
    let addr = serve(move |target, _| {
        if target.starts_with("/github/releases") {
            hits_clone.fetch_add(1, Ordering::SeqCst);
            return http_status(
                "HTTP/1.1 429 Too Many Requests",
                &[("Retry-After", "60".to_string())],
                b"slow down",
            );
        }
        not_found()
    });
    let mut fixture = fixture(addr);
    fixture.config.scan_budget = Some(Duration::from_millis(300));
    // Cap raised above the remaining budget so the wanted 60s sleep genuinely
    // exceeds it: the scan must stop instead of sleeping past the deadline.
    fixture.config.retry_after_cap = Duration::from_secs(60);
    fixture.client = discovery_client(
        &fixture.config,
        &combined_origin_allowlist(&fixture.endpoints),
    )
    .unwrap();
    let started = Instant::now();
    match run(&fixture, ReleaseSource::GitHub) {
        DiscoveryOutcome::NoEligibleCandidate { scans } => {
            assert!(scans[0].budget_exhausted);
        }
        other => panic!("expected NoEligibleCandidate, got {other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(hits.load(Ordering::SeqCst), 1, "no retry past the deadline");
}

/// Main/helper trust parity (Phase 3A): the main application's runtime
/// trust store is the shared compiled production store — there is no local
/// override anywhere in the updater module. The store it observes must
/// equal the shared crate's own view of the provisioned keys, entry for
/// entry; today both are empty (fail-closed until provisioning), and after
/// provisioning both sides must observe exactly the same key ids. The
/// maintenance helper carries the mirror-image test.
#[test]
fn main_trust_store_is_the_shared_compiled_production_store() {
    let store = desktop_todo_update_core::production_trust_store();
    let store_ids: Vec<String> = store
        .entries()
        .iter()
        .map(|entry| entry.key_id().to_string())
        .collect();
    assert_eq!(store_ids, desktop_todo_update_core::production_key_ids());
    // Same answer on repeated construction: compiled data, not per-process
    // state.
    let again = desktop_todo_update_core::production_trust_store();
    assert_eq!(store.len(), again.len());
    for (entry, other) in store.entries().iter().zip(again.entries()) {
        assert_eq!(entry.key_id(), other.key_id());
        assert_eq!(entry.raw_public_key(), other.raw_public_key());
    }
}
