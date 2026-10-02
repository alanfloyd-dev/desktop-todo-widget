//! Integration-style tests for the publish/read-back layer (Phase 3B): the
//! real reqwest-backed GitHub/Gitee publishers exercised against local mock
//! servers emulating the two REST shapes. No network access, no real
//! release mutation.
//!
//! Lives inside the lib (not `tests/`) deliberately: this crate is a path
//! dependency of the canonical parent manifest and is not a workspace
//! member, so dev-dependencies cannot be resolved for it — the same
//! constraint that keeps the maintenance crate's test fixtures as regular
//! dependencies.
//!
//! **TEST-ONLY key material** via the offline signer's `generate-keypair`.

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::pipeline::PrepareOptions;
use crate::publish::{
    execute_publish, plan_publish, read_back, assert_mirror_consistency, ArtifactSet,
    GitHubPublisher, GiteePublisher, ProviderApi, PublishDecision,
};
use crate::{facts::ReleaseFacts, ReleaseMode, MANIFEST_ASSET_NAME};

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "release-pipeline-publish-{}-{}-{label}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

// ------------------------------------------------------------ mock provider

#[derive(Default)]
struct MockState {
    release: Option<(u64, bool)>, // (id, draft)
    assets: Vec<(String, Vec<u8>)>,
    /// Absolute base for the download URLs the mock hands out (real
    /// providers always return absolute URLs).
    base: String,
}

type SharedState = Arc<Mutex<MockState>>;

fn http_response(status: &str, body: &[u8]) -> Vec<u8> {
    let mut bytes = format!(
        "{status}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

fn release_json(state: &MockState, tag: &str) -> Option<serde_json::Value> {
    let (id, draft) = state.release?;
    let assets: Vec<serde_json::Value> = state
        .assets
        .iter()
        .map(|(name, bytes)| {
            serde_json::json!({
                "name": name,
                "size": bytes.len(),
                "browser_download_url": format!("{}/download/{name}", state.base),
            })
        })
        .collect();
    Some(serde_json::json!({
        "id": id,
        "tag_name": tag,
        "draft": draft,
        "prerelease": false,
        "upload_url": format!("/uploads/o/r/releases/{id}/assets{{?name,label}}"),
        "assets": assets,
    }))
}

/// One mock server emulating the GitHub and Gitee REST shapes the
/// publishers speak. Reads full request bodies (Content-Length framed).
fn spawn_mock(state: SharedState, tag: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    state.lock().unwrap().base = format!("http://{addr}");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let state = state.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream);
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    head.push_str(&line);
                }
                let mut lines = head.lines();
                let request_line = lines.next().unwrap_or("").to_string();
                let mut content_length = 0usize;
                for line in lines {
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        content_length = value.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; content_length];
                if content_length > 0 {
                    reader.read_exact(&mut body).unwrap();
                }
                let mut parts = request_line.split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let target = parts.next().unwrap_or("/").to_string();

                let mut st = state.lock().unwrap();
                let response = match (method.as_str(), target.as_str()) {
                    ("GET", t) if t.starts_with("/repos/o/r/releases/tags/") => {
                        // Real GitHub shape (Phase 4A-1): a DRAFT release is
                        // not reachable by tag — the tag ref exists only
                        // after publish — so a draft 404s here and must be
                        // found through the releases list instead.
                        match release_json(&st, tag) {
                            Some(value)
                                if !value
                                    .get("draft")
                                    .and_then(|d| d.as_bool())
                                    .unwrap_or(false) =>
                            {
                                http_response("HTTP/1.1 200 OK", &value.to_string().into_bytes())
                            }
                            _ => http_response("HTTP/1.1 404 Not Found", b"missing"),
                        }
                    }
                    ("GET", t)
                        if t.starts_with("/repos/o/r/releases")
                            && !t.contains("/tags/")
                            && !t.contains("/attach_files") =>
                    {
                        // The authenticated list includes the caller's
                        // drafts (the draft-resolution fallback path).
                        match release_json(&st, tag) {
                            Some(value) => http_response(
                                "HTTP/1.1 200 OK",
                                format!("[{value}]").into_bytes().as_slice(),
                            ),
                            None => http_response("HTTP/1.1 200 OK", b"[]"),
                        }
                    }
                    ("GET", t) if t.starts_with("/api/v5/repos/o/r/releases/tags/") => {
                        match release_json(&st, tag) {
                            Some(value) => http_response("HTTP/1.1 200 OK", &value.to_string().into_bytes()),
                            // Real Gitee shape (Phase 4A-2): a missing release
                            // is HTTP 200 with a literal `null` body.
                            None => http_response("HTTP/1.1 200 OK", b"null"),
                        }
                    }
                    ("GET", t)
                        if t.starts_with("/api/v5/repos/o/r") && !t.contains("/releases") =>
                    {
                        // Real Gitee shape: the repository endpoint supplies
                        // the default branch used as `target_commitish`.
                        http_response(
                            "HTTP/1.1 200 OK",
                            br#"{"id":1,"full_name":"o/r","default_branch":"main"}"#,
                        )
                    }
                    ("POST", t) if t.starts_with("/repos/o/r/releases") && !t.contains("/attach_files") => {
                        if st.release.is_some() {
                            http_response("HTTP/1.1 422 Unprocessable", b"exists")
                        } else {
                            let draft = serde_json::from_slice::<serde_json::Value>(&body)
                                .ok()
                                .and_then(|v| v.get("draft").and_then(|d| d.as_bool()))
                                .unwrap_or(false);
                            st.release = Some((1, draft));
                            let value = release_json(&st, tag).unwrap();
                            http_response("HTTP/1.1 201 Created", &value.to_string().into_bytes())
                        }
                    }
                    ("POST", t)
                        if t.starts_with("/api/v5/repos/o/r/releases")
                            && !t.contains("/attach_files") =>
                    {
                        if st.release.is_some() {
                            http_response("HTTP/1.1 422 Unprocessable", b"exists")
                        } else {
                            let payload = serde_json::from_slice::<serde_json::Value>(&body)
                                .ok()
                                .unwrap_or_default();
                            // Real Gitee shape (Phase 4A-2): creating a release
                            // for a not-yet-existing tag requires
                            // `target_commitish`; refuse without it.
                            if payload
                                .get("target_commitish")
                                .and_then(|v| v.as_str())
                                .map(|s| !s.is_empty())
                                != Some(true)
                            {
                                http_response(
                                    "HTTP/1.1 400 Bad Request",
                                    br#"{"messages":["target_commitish is missing"]}"#,
                                )
                            } else {
                                let draft = payload
                                    .get("draft")
                                    .and_then(|d| d.as_bool())
                                    .unwrap_or(false);
                                st.release = Some((1, draft));
                                let value = release_json(&st, tag).unwrap();
                                http_response("HTTP/1.1 201 Created", &value.to_string().into_bytes())
                            }
                        }
                    }
                    ("POST", t) if t.starts_with("/uploads/o/r/releases/") => {
                        let name = target
                            .split("name=")
                            .nth(1)
                            .unwrap_or("unknown")
                            .to_string();
                        st.assets.retain(|(n, _)| n != &name);
                        st.assets.push((name, body));
                        http_response("HTTP/1.1 201 Created", b"{}")
                    }
                    ("POST", t) if t.starts_with("/api/v5/repos/o/r/releases/1/attach_files") => {
                        let marker = "filename=\"";
                        let name = body
                            .windows(marker.len())
                            .position(|w| w == marker.as_bytes())
                            .map(|i| {
                                let rest = &body[i + marker.len()..];
                                let end = rest.iter().position(|b| *b == b'"').unwrap();
                                String::from_utf8_lossy(&rest[..end]).to_string()
                            })
                            .unwrap_or_else(|| "unknown".to_string());
                        let header_end = body.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
                        let payload = &body[header_end + 4..];
                        // Cut at the closing boundary marker.
                        let payload = match payload
                            .windows(4)
                            .rposition(|w| w == b"\r\n--")
                        {
                            Some(index) => &payload[..index],
                            None => payload,
                        };
                        st.assets.retain(|(n, _)| n != &name);
                        st.assets.push((name, payload.to_vec()));
                        http_response("HTTP/1.1 201 Created", b"{}")
                    }
                    ("GET", t) if t.starts_with("/download/") => {
                        let name = t.trim_start_matches("/download/");
                        match st.assets.iter().find(|(n, _)| n == name) {
                            Some((_, bytes)) => http_response("HTTP/1.1 200 OK", bytes),
                            None => http_response("HTTP/1.1 404 Not Found", b"missing"),
                        }
                    }
                    ("PATCH", t) if t.starts_with("/repos/o/r/releases/") => {
                        if let Some((_, draft)) = st.release.as_mut() {
                            *draft = serde_json::from_slice::<serde_json::Value>(&body)
                                .ok()
                                .and_then(|v| v.get("draft").and_then(|d| d.as_bool()))
                                .unwrap_or(*draft);
                        }
                        http_response("HTTP/1.1 200 OK", b"{}")
                    }
                    _ => http_response("HTTP/1.1 404 Not Found", b"no route"),
                };
                drop(st);
                use std::io::Write;
                let _ = reader.get_mut().write_all(&response);
                let _ = reader.get_mut().flush();
            });
        }
    });
    format!("http://{addr}")
}

// ------------------------------------------------------------ fixtures

fn ensure_tokens() {
    std::env::set_var("DTW_GITHUB_TOKEN", "test-token-rehearsal-only");
    std::env::set_var("DTW_GITEE_TOKEN", "test-token-rehearsal-only");
}

/// A fully prepared rehearsal staging root (through the real prepare flow).
fn prepared_staging(label: &str) -> (PathBuf, PathBuf) {
    let repo = temp_dir(label);
    fs::create_dir_all(repo.join("src-tauri/target/release")).unwrap();
    for (name, bytes) in [
        ("install.ps1", &b"install\n"[..]),
        ("uninstall.ps1", &b"uninstall\n"[..]),
        ("README.md", &b"r\n"[..]),
        ("README_ZH.md", &b"rz\n"[..]),
        ("LICENSE", &b"M\n"[..]),
        ("LICENSE_ZH.md", &b"Mz\n"[..]),
        ("THIRD_PARTY_NOTICES.md", &b"n\n"[..]),
    ] {
        fs::write(repo.join(name), bytes).unwrap();
    }
    fs::write(
        repo.join("src-tauri/target/release/alan-desktop.exe"),
        vec![1u8; 128],
    )
    .unwrap();
    fs::write(
        repo.join("src-tauri/target/release/desktop-todo-maintenance.exe"),
        vec![2u8; 96],
    )
    .unwrap();
    let key = repo.join("test.seed");
    let public = repo.join("test.pub");
    desktop_todo_release_signer::generate_keypair(&key, &public).unwrap();
    let staging = repo.join("staging");
    fs::create_dir_all(&staging).unwrap();
    let options = PrepareOptions {
        staging: staging.clone(),
        repo_root: repo.clone(),
        main_exe: None,
        helper_exe: None,
        version: "1.2.0".to_string(),
        published_at: "2026-10-02T00:00:00Z".to_string(),
        notes: "Application lifecycle management.".to_string(),
        mode: ReleaseMode::Rehearsal,
        key_path: Some(key.clone()),
        trust_public_path: Some(public.clone()),
        source_commit: Some("258d660".to_string()),
        overwrite: false,
    };
    crate::pipeline::prepare(&options, false).unwrap();
    (repo, staging)
}

fn load_facts(staging: &Path) -> ReleaseFacts {
    crate::report::read_facts(staging).unwrap()
}

fn rehearsal_trust(repo: &Path) -> desktop_todo_update_core::TrustStore {
    crate::signing::verification_trust(ReleaseMode::Rehearsal, Some(&repo.join("test.pub")))
        .unwrap()
}

fn mock_publisher_github(base: &str) -> Box<dyn ProviderApi> {
    Box::new(
        GitHubPublisher::for_rehearsal("o/r", base.to_string(), base.to_string()).unwrap(),
    )
}

fn mock_publisher_gitee(base: &str) -> Box<dyn ProviderApi> {
    Box::new(GiteePublisher::for_rehearsal("o/r", format!("{base}/api/v5")).unwrap())
}

// ------------------------------------------------------------ tests



    #[test]
    fn publish_is_idempotent_for_identical_assets_and_refuses_different_bytes() {
        ensure_tokens();
        let (repo, staging) = prepared_staging("idempotent");
        let facts = load_facts(&staging);
        let artifacts = ArtifactSet::from_staging(&staging, &facts);
        let tag = "v1.2.0";

        let state: SharedState = Arc::new(Mutex::new(MockState::default()));
        let base = spawn_mock(state.clone(), "v1.2.0");
        let api = mock_publisher_github(&base);

        let (release, decisions) =
            execute_publish(api.as_ref(), &artifacts, tag, "title", "body", ReleaseMode::Rehearsal)
                .unwrap();
        assert!(
            decisions
                .iter()
                .any(|d| matches!(d, PublishDecision::CreateRelease { draft: true }))
        );
        assert_eq!(
            decisions
                .iter()
                .filter(|d| matches!(d, PublishDecision::UploadAsset { .. }))
                .count(),
            4
        );
        assert!(release.draft);
        {
            let st = state.lock().unwrap();
            assert_eq!(st.assets.len(), 4);
            let manifest = st
                .assets
                .iter()
                .find(|(n, _)| n == MANIFEST_ASSET_NAME)
                .unwrap();
            assert_eq!(manifest.1, fs::read(&artifacts.manifest).unwrap());
        }

        let (_, decisions) =
            execute_publish(api.as_ref(), &artifacts, tag, "title", "body", ReleaseMode::Rehearsal)
                .unwrap();
        assert_eq!(
            decisions
                .iter()
                .filter(|d| matches!(d, PublishDecision::IdempotentAsset { .. }))
                .count(),
            4
        );

        let tampered_dir = temp_dir("idempotent-tampered");
        let tampered_artifacts = ArtifactSet {
            package: artifacts.package.clone(),
            manifest: tampered_dir.join(MANIFEST_ASSET_NAME),
            envelope: artifacts.envelope.clone(),
            sidecar: artifacts.sidecar.clone(),
        };
        fs::write(&tampered_artifacts.manifest, b"tampered-bytes").unwrap();
        let error = execute_publish(
            api.as_ref(),
            &tampered_artifacts,
            tag,
            "title",
            "body",
            ReleaseMode::Rehearsal,
        )
        .unwrap_err();
        assert!(matches!(error, crate::PipelineError::PublishCollision { .. }), "{error}");
        {
            let st = state.lock().unwrap();
            let manifest = st
                .assets
                .iter()
                .find(|(n, _)| n == MANIFEST_ASSET_NAME)
                .unwrap();
            assert_ne!(manifest.1, b"tampered-bytes".to_vec());
        }
        fs::remove_dir_all(&repo).ok();
        fs::remove_dir_all(&tampered_dir).ok();
    }

    #[test]
    fn plan_publish_does_not_mutate_the_provider() {
        ensure_tokens();
        let (repo, staging) = prepared_staging("plan");
        let facts = load_facts(&staging);
        let artifacts = ArtifactSet::from_staging(&staging, &facts);
        let state: SharedState = Arc::new(Mutex::new(MockState::default()));
        let base = spawn_mock(state.clone(), "v1.2.0");
        let api = mock_publisher_github(&base);

        let (existing, decisions) =
            plan_publish(api.as_ref(), &artifacts, "v1.2.0", ReleaseMode::Rehearsal).unwrap();
        assert!(existing.is_none());
        assert!(matches!(decisions[0], PublishDecision::CreateRelease { draft: true }));
        assert!(state.lock().unwrap().release.is_none());
        fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn partial_upload_is_observed_as_a_collision() {
        ensure_tokens();
        let (repo, staging) = prepared_staging("partial");
        let facts = load_facts(&staging);
        let artifacts = ArtifactSet::from_staging(&staging, &facts);

        let state: SharedState = Arc::new(Mutex::new(MockState {
            release: Some((7, true)),
            assets: vec![(facts.package_filename.clone(), vec![0u8; 10])],
            ..MockState::default()
        }));
        let base = spawn_mock(state.clone(), "v1.2.0");
        let api = mock_publisher_github(&base);
        let error = execute_publish(
            api.as_ref(),
            &artifacts,
            "v1.2.0",
            "t",
            "b",
            ReleaseMode::Rehearsal,
        )
        .unwrap_err();
        assert!(matches!(error, crate::PipelineError::PublishCollision { .. }), "{error}");
        fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn read_back_passes_and_detects_remote_tampering() {
        ensure_tokens();
        let (repo, staging) = prepared_staging("readback");
        let facts = load_facts(&staging);
        let artifacts = ArtifactSet::from_staging(&staging, &facts);
        let trust = rehearsal_trust(&repo);

        let state: SharedState = Arc::new(Mutex::new(MockState::default()));
        let base = spawn_mock(state.clone(), "v1.2.0");
        let api = mock_publisher_github(&base);
        execute_publish(api.as_ref(), &artifacts, "v1.2.0", "t", "b", ReleaseMode::Rehearsal).unwrap();

        let result = read_back(api.as_ref(), &artifacts, &facts, &trust, &staging, "v1.2.0").unwrap();
        assert_eq!(result.provider, "github");
        assert_eq!(result.artifact_sha256.len(), 4);

        let mut tampered = fs::read(&artifacts.manifest).unwrap();
        tampered[0] ^= 0xFF;
        {
            let mut st = state.lock().unwrap();
            let entry = st
                .assets
                .iter_mut()
                .find(|(n, _)| n == MANIFEST_ASSET_NAME)
                .unwrap();
            entry.1 = tampered;
        }
        let error = read_back(api.as_ref(), &artifacts, &facts, &trust, &staging, "v1.2.0").unwrap_err();
        assert!(matches!(error, crate::PipelineError::ReadBack { .. }), "{error}");
        assert!(!staging.join("read-back-temp").exists());
        fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn read_back_rehearsal_envelope_never_verifies_under_production_trust() {
        ensure_tokens();
        let (repo, staging) = prepared_staging("trustsep");
        let facts = load_facts(&staging);
        let artifacts = ArtifactSet::from_staging(&staging, &facts);

        let state: SharedState = Arc::new(Mutex::new(MockState::default()));
        let base = spawn_mock(state.clone(), "v1.2.0");
        let api = mock_publisher_github(&base);
        execute_publish(api.as_ref(), &artifacts, "v1.2.0", "t", "b", ReleaseMode::Rehearsal).unwrap();

        let production = crate::signing::verification_trust(ReleaseMode::Production, None).unwrap();
        let error = read_back(api.as_ref(), &artifacts, &facts, &production, &staging, "v1.2.0").unwrap_err();
        assert!(matches!(error, crate::PipelineError::ReadBack { .. }), "{error}");
        fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn mirror_disagreement_fails_the_gate() {
        ensure_tokens();
        let (repo, staging) = prepared_staging("mirror");
        let facts = load_facts(&staging);
        let artifacts = ArtifactSet::from_staging(&staging, &facts);
        let trust = rehearsal_trust(&repo);

        let github_state: SharedState = Arc::new(Mutex::new(MockState::default()));
        let gitee_state: SharedState = Arc::new(Mutex::new(MockState::default()));
        let github_base = spawn_mock(github_state.clone(), "v1.2.0");
        let gitee_base = spawn_mock(gitee_state.clone(), "v1.2.0");
        let github = mock_publisher_github(&github_base);
        let gitee = mock_publisher_gitee(&gitee_base);

        execute_publish(github.as_ref(), &artifacts, "v1.2.0", "t", "b", ReleaseMode::Rehearsal).unwrap();
        execute_publish(gitee.as_ref(), &artifacts, "v1.2.0", "t", "b", ReleaseMode::Rehearsal).unwrap();

        let left = read_back(github.as_ref(), &artifacts, &facts, &trust, &staging, "v1.2.0").unwrap();
        let right = read_back(gitee.as_ref(), &artifacts, &facts, &trust, &staging, "v1.2.0").unwrap();
        assert_mirror_consistency(&left, &right).unwrap();

        {
            let mut st = gitee_state.lock().unwrap();
            let entry = st
                .assets
                .iter_mut()
                .find(|(n, _)| n == MANIFEST_ASSET_NAME)
                .unwrap();
            entry.1[0] ^= 0xFF;
        }
        let error = read_back(gitee.as_ref(), &artifacts, &facts, &trust, &staging, "v1.2.0").unwrap_err();
        assert!(matches!(error, crate::PipelineError::ReadBack { .. }), "{error}");
        fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn gitee_rehearsal_on_the_production_endpoint_requires_prerelease_isolation() {
        ensure_tokens();
        // Constructed, never used for I/O: the isolation gate is a pure
        // predicate over the provider endpoint and the release visibility
        // state. Gitee has no draft state, so the discovery-equivalent
        // isolation is the prerelease publication flag.
        let api: Box<dyn ProviderApi> = Box::new(GiteePublisher::production("o/r").unwrap());
        let published = crate::publish::RemoteRelease {
            id: "1".to_string(),
            tag: "updater-rehearsal-gitee-test".to_string(),
            draft: false,
            prerelease: false,
            assets: Vec::new(),
            upload_url: None,
        };
        // A publication-visible release must never carry rehearsal artifacts.
        let error = crate::publish::enforce_rehearsal_visibility_isolation(
            api.as_ref(),
            &published,
            ReleaseMode::Rehearsal,
        )
        .unwrap_err();
        assert!(matches!(error, crate::PipelineError::RehearsalSafety { .. }), "{error}");
        // A prerelease-marked release is the one permitted Gitee channel.
        let prerelease = crate::publish::RemoteRelease {
            prerelease: true,
            ..published.clone()
        };
        crate::publish::enforce_rehearsal_visibility_isolation(
            api.as_ref(),
            &prerelease,
            ReleaseMode::Rehearsal,
        )
        .unwrap();
        // Production mode is never restricted by rehearsal gates.
        crate::publish::enforce_rehearsal_visibility_isolation(
            api.as_ref(),
            &published,
            ReleaseMode::Production,
        )
        .unwrap();
        // And a non-production endpoint never restricts rehearsal.
        let mock: Box<dyn ProviderApi> = Box::new(
            GiteePublisher::for_rehearsal("o/r", "http://127.0.0.1:1/api/v5".to_string()).unwrap(),
        );
        crate::publish::enforce_rehearsal_visibility_isolation(
            mock.as_ref(),
            &published,
            ReleaseMode::Rehearsal,
        )
        .unwrap();
    }

    #[test]
    fn github_rehearsal_on_the_production_endpoint_requires_draft_isolation() {
        ensure_tokens();
        // Constructed, never used for I/O: the isolation gate is a pure
        // predicate over the provider endpoint and the release draft state.
        let api: Box<dyn ProviderApi> = Box::new(GitHubPublisher::production("o/r").unwrap());
        let published = crate::publish::RemoteRelease {
            id: "1".to_string(),
            tag: "v0.0.903".to_string(),
            draft: false,
            prerelease: true,
            assets: Vec::new(),
            upload_url: None,
        };
        // A published release must never carry rehearsal artifacts.
        let error = crate::publish::enforce_rehearsal_visibility_isolation(
            api.as_ref(),
            &published,
            ReleaseMode::Rehearsal,
        )
        .unwrap_err();
        assert!(matches!(error, crate::PipelineError::RehearsalSafety { .. }), "{error}");
        // A draft is the one permitted channel.
        let draft = crate::publish::RemoteRelease {
            draft: true,
            ..published.clone()
        };
        crate::publish::enforce_rehearsal_visibility_isolation(api.as_ref(), &draft, ReleaseMode::Rehearsal)
            .unwrap();
        // Production mode is never restricted by rehearsal gates.
        crate::publish::enforce_rehearsal_visibility_isolation(
            api.as_ref(),
            &published,
            ReleaseMode::Production,
        )
        .unwrap();
        // And a non-production endpoint never restricts rehearsal.
        let mock: Box<dyn ProviderApi> = Box::new(
            GitHubPublisher::for_rehearsal(
                "o/r",
                "http://127.0.0.1:1".to_string(),
                "http://127.0.0.1:1".to_string(),
            )
            .unwrap(),
        );
        crate::publish::enforce_rehearsal_visibility_isolation(
            mock.as_ref(),
            &published,
            ReleaseMode::Rehearsal,
        )
        .unwrap();
    }

    #[test]
    fn finalize_is_refused_for_rehearsal_on_production_endpoints() {
        ensure_tokens();
        let api: Box<dyn ProviderApi> = Box::new(GitHubPublisher::production("o/r").unwrap());
        let error =
            crate::publish::enforce_finalize_safety(api.as_ref(), ReleaseMode::Rehearsal)
                .unwrap_err();
        assert!(matches!(error, crate::PipelineError::RehearsalSafety { .. }), "{error}");
        // A non-production endpoint is unrestricted (mock finalize stays
        // testable), and production mode on the production endpoint is the
        // legitimate finalize use.
        let mock: Box<dyn ProviderApi> = Box::new(
            GitHubPublisher::for_rehearsal(
                "o/r",
                "http://127.0.0.1:1".to_string(),
                "http://127.0.0.1:1".to_string(),
            )
            .unwrap(),
        );
        crate::publish::enforce_finalize_safety(mock.as_ref(), ReleaseMode::Rehearsal).unwrap();
        crate::publish::enforce_finalize_safety(api.as_ref(), ReleaseMode::Production).unwrap();
    }

    #[test]
    fn gitee_publish_round_trips_and_has_no_draft_state() {
        ensure_tokens();
        let (repo, staging) = prepared_staging("gitee");
        let facts = load_facts(&staging);
        let artifacts = ArtifactSet::from_staging(&staging, &facts);
        let trust = rehearsal_trust(&repo);

        let state: SharedState = Arc::new(Mutex::new(MockState::default()));
        let base = spawn_mock(state.clone(), "v1.2.0");
        let api = mock_publisher_gitee(&base);

        // Real Gitee shape regression (Phase 4A-2): a tag with no release
        // resolves to HTTP 200 with a literal `null` body, which find_release
        // must map to None — never a parse failure.
        assert!(api.find_release("v1.2.0").unwrap().is_none());

        let (release, decisions) =
            execute_publish(api.as_ref(), &artifacts, "v1.2.0", "t", "b", ReleaseMode::Rehearsal)
                .unwrap();
        assert_eq!(
            decisions
                .iter()
                .filter(|d| matches!(d, PublishDecision::UploadAsset { .. }))
                .count(),
            4
        );
        read_back(api.as_ref(), &artifacts, &facts, &trust, &staging, "v1.2.0").unwrap();
        let error = api.set_publication(&release, false).unwrap_err();
        assert!(matches!(error, crate::PipelineError::Publish { .. }), "{error}");
        fs::remove_dir_all(&repo).ok();
    }
