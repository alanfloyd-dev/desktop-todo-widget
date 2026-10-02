//! Provider publishing, read-back verification, and mirror consistency
//! (Phase 3B, pipeline steps 5–7).
//!
//! Layering (frozen for this round): **publish is three operations** —
//! local prepare (already done by the time this module runs), provider
//! publish, provider read-back verify. This module only ever touches the
//! public artifact set (package ZIP, manifest, envelope, `.sha256` sidecar);
//! it cannot compute trust facts, never sees a signing key, and never
//! reserializes a signed document. Provider tokens are provider auth only —
//! read from environment variables, never logged, never written to reports.
//!
//! Collision/idempotence policy (§18 of the round, explicit):
//!
//! - release missing → create (GitHub: **draft** — invisible to stable
//!   discovery until explicitly finalized; Gitee has no draft state, so a
//!   Gitee publish is immediately visible and the tool reports that
//!   honestly);
//! - asset missing → upload, then immediately read the uploaded asset back
//!   and compare digests;
//! - asset present with identical bytes → idempotent success (no rewrite);
//! - asset present with different bytes → **fail closed**; no silent
//!   overwrite of published signed artifacts ever (remediation is an
//!   explicit operator deletion on the provider);
//! - partial previous upload → observed as different bytes → fail closed.
//!
//! Rehearsal safety (visibility-isolation invariant): rehearsal artifacts
//! may only ever touch a production endpoint through a release that stable
//! updater discovery will not select. Concretely: GitHub rehearsal requires
//! a **draft** release (invisible to discovery entirely) — a published
//! release carrying rehearsal material is a safety violation, and
//! finalizing a rehearsal release on the production endpoint is refused.
//! Gitee has no draft state (a created release is immediately visible), so
//! its rehearsal isolation is the provider's **prerelease** publication
//! flag — the exact field the discovery adapters' publication-eligibility
//! step filters on — chosen with an explicitly non-production version
//! identity. Non-production endpoints (local mocks, isolated test
//! endpoints) are unrestricted.
//!
//! Publication eligibility (§16 of application-lifecycle): a release is
//! exposed to updater discovery only after read-back verification — the
//! GitHub flow creates the release as a draft, uploads, verifies by
//! read-back, and only then flips `draft=false` via `finalize`.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::facts::ReleaseFacts;
use crate::{ReleaseMode, PipelineError};

/// The production publish target — the same repository identity the
/// discovery adapters compile (providers.rs); re-reviewed at release time.
pub const PRODUCTION_REPO: &str = "alanfloyd-dev/desktop-todo-widget";

/// Which provider a publish/read-back run targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    GitHub,
    Gitee,
}

impl Provider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::GitHub => "github",
            Provider::Gitee => "gitee",
        }
    }
    /// Environment variable carrying the provider token (provider auth
    /// only — fully separate from signing material, never logged).
    pub fn token_env_var(&self) -> &'static str {
        match self {
            Provider::GitHub => "DTW_GITHUB_TOKEN",
            Provider::Gitee => "DTW_GITEE_TOKEN",
        }
    }
}

/// The exact four artifacts one protocol release publishes: the three
/// frozen discovery assets (package ZIP, `update-manifest.json`,
/// `update-manifest.json.sig`) plus the manual-bootstrap `.sha256` sidecar
/// (release requirements §16.3; not a discovery asset).
#[derive(Debug, Clone)]
pub struct ArtifactSet {
    pub package: PathBuf,
    pub manifest: PathBuf,
    pub envelope: PathBuf,
    pub sidecar: PathBuf,
}

impl ArtifactSet {
    /// Resolve the artifact set from the staging root and the release facts
    /// (filenames come from facts, never from a directory scan).
    pub fn from_staging(staging: &Path, facts: &ReleaseFacts) -> ArtifactSet {
        ArtifactSet {
            package: staging.join(&facts.package_filename),
            manifest: staging.join(crate::MANIFEST_ASSET_NAME),
            envelope: staging.join(crate::ENVELOPE_ASSET_NAME),
            sidecar: staging.join(format!(
                "{}{}",
                facts.package_filename,
                crate::SIDECAR_SUFFIX
            )),
        }
    }

    /// (name, path) pairs in upload order: metadata first, then the package
    /// (so a partial run most likely leaves the package — the largest and
    /// last — missing, which the collision policy reports explicitly).
    pub fn ordered(&self) -> Vec<(String, &Path)> {
        vec![
            (
                crate::MANIFEST_ASSET_NAME.to_string(),
                self.manifest.as_path(),
            ),
            (
                crate::ENVELOPE_ASSET_NAME.to_string(),
                self.envelope.as_path(),
            ),
            (self.package_filename_name(), self.package.as_path()),
            (self.sidecar_name(), self.sidecar.as_path()),
        ]
    }

    fn package_filename_name(&self) -> String {
        self.package
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    fn sidecar_name(&self) -> String {
        self.sidecar
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    }
}

/// One remote release asset (provider-reported; never trusted — every byte
/// is verified at read-back).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteAsset {
    pub name: String,
    pub size: Option<u64>,
    pub url: String,
}

/// Provider-reported release record (untrusted discovery-shaped data).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRelease {
    pub id: String,
    pub tag: String,
    pub draft: bool,
    pub prerelease: bool,
    pub assets: Vec<RemoteAsset>,
    /// The provider's asset-upload URL template (GitHub only).
    pub upload_url: Option<String>,
}

/// The narrow provider API surface. Real implementations speak the GitHub
/// REST / Gitee v5 APIs; tests exercise the same real implementations
/// against a local mock server.
pub trait ProviderApi {
    fn find_release(&self, tag: &str) -> Result<Option<RemoteRelease>, PipelineError>;
    fn create_release(
        &self,
        tag: &str,
        title: &str,
        body: &str,
        prerelease: bool,
        draft: bool,
    ) -> Result<RemoteRelease, PipelineError>;
    fn upload_asset(&self, release: &RemoteRelease, name: &str, path: &Path)
        -> Result<(), PipelineError>;
    /// Flip the release between draft and published. Providers without a
    /// draft state return a typed failure (never a silent no-op).
    fn set_publication(&self, release: &RemoteRelease, draft: bool) -> Result<(), PipelineError>;
    /// Download exact remote bytes to the given writer (streaming).
    fn download(&self, url: &str, writer: &mut dyn Write) -> Result<(), PipelineError>;
    /// Whether this instance points at the compiled production endpoints.
    fn is_production_endpoint(&self) -> bool;
    fn provider(&self) -> Provider;
}

// ---------------------------------------------------------------- transport

/// Shared blocking client for the publishing tool. Deliberately declared
/// without any decompression features (same as the product's updater
/// client): read-back must always compare the exact served bytes.
fn pipeline_client() -> Result<reqwest::blocking::Client, PipelineError> {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(600))
        .user_agent(concat!(
            "desktop-todo-release-pipeline/",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .map_err(|e| PipelineError::Publish {
            detail: format!("cannot build the publishing HTTP client: {e}"),
        })
}

fn read_token(provider: Provider) -> Result<String, PipelineError> {
    std::env::var(provider.token_env_var())
        .ok()
        .filter(|token| !token.trim().is_empty())
        .ok_or(PipelineError::TokenMissing {
            provider: provider.as_str(),
            env_var: provider.token_env_var(),
        })
}

/// JSON helpers: fail closed on unexpected provider shapes instead of
/// guessing.
fn json_field<'a>(value: &'a serde_json::Value, field: &str) -> Result<&'a serde_json::Value, PipelineError> {
    Ok(value.get(field).ok_or(PipelineError::Publish {
        detail: format!("provider response is missing the {field:?} field"),
    })?)
}

fn json_string(value: &serde_json::Value, field: &str) -> Result<String, PipelineError> {
    match json_field(value, field)? {
        serde_json::Value::String(text) => Ok(text.clone()),
        other => Ok(other.to_string()),
    }
}

/// The URL the pipeline downloads an asset's exact bytes from.
///
/// Real GitHub shape (Phase 4A-1 smoke): a draft release's
/// `browser_download_url` is an `untagged-…` URL that does not serve asset
/// bytes to an API token (HTTP 404). The API asset URL (`url` field) is the
/// authenticated exact-bytes download endpoint for both drafts and
/// published releases when requested with `Accept: application/octet-stream`
/// — which `download` always sends. The `browser_download_url` remains the
/// fallback (local mock fixtures carry only that field) and the Gitee form.
fn asset_download_url(
    entry: &serde_json::Value,
    provider: Provider,
) -> Result<String, PipelineError> {
    match provider {
        Provider::GitHub => entry
            .get("url")
            .and_then(|u| u.as_str())
            .map(|u| u.to_string())
            .or_else(|| {
                entry
                    .get("browser_download_url")
                    .and_then(|u| u.as_str())
                    .map(|u| u.to_string())
            })
            .ok_or(PipelineError::Publish {
                detail: "asset carries no downloadable url".to_string(),
            }),
        Provider::Gitee => json_string(entry, "browser_download_url"),
    }
}

fn parse_release(value: &serde_json::Value, provider: Provider) -> Result<RemoteRelease, PipelineError> {
    let id = match provider {
        Provider::GitHub => json_string(value, "id")?,
        Provider::Gitee => match value.get("id") {
            Some(serde_json::Value::Number(number)) => number.to_string(),
            _ => json_string(value, "id")?,
        },
    };
    let assets = match value.get("assets") {
        Some(serde_json::Value::Array(entries)) => entries
            .iter()
            .map(|entry| {
                Ok(RemoteAsset {
                    name: json_string(entry, "name")?,
                    size: entry.get("size").and_then(|s| s.as_u64()),
                    url: asset_download_url(entry, provider)?,
                })
            })
            .collect::<Result<Vec<RemoteAsset>, PipelineError>>()?,
        _ => Vec::new(),
    };
    Ok(RemoteRelease {
        id,
        tag: json_string(value, "tag_name")?,
        draft: value.get("draft").and_then(|d| d.as_bool()).unwrap_or(false),
        prerelease: value
            .get("prerelease")
            .and_then(|p| p.as_bool())
            .unwrap_or(false),
        assets,
        upload_url: value
            .get("upload_url")
            .and_then(|u| u.as_str())
            .map(|u| u.to_string()),
    })
}

// ---------------------------------------------------------------- GitHub

/// GitHub REST API publisher. `api_base`/`upload_base` exist for isolated
/// rehearsal endpoints and tests; production defaults to the real hosts.
pub struct GitHubPublisher {
    client: reqwest::blocking::Client,
    repo: String,
    token: String,
    api_base: String,
    upload_base: String,
}

impl GitHubPublisher {
    /// Production publisher: compiled repo + real endpoints. Rehearsal use
    /// requires explicit endpoint overrides (`for_rehearsal`).
    pub fn production(repo: &str) -> Result<Self, PipelineError> {
        Self::new(
            repo,
            "https://api.github.com".to_string(),
            "https://uploads.github.com".to_string(),
        )
    }

    /// Isolated rehearsal publisher (explicit endpoints; still requires a
    /// token from the same env var — the endpoint is the isolation).
    pub fn for_rehearsal(repo: &str, api_base: String, upload_base: String) -> Result<Self, PipelineError> {
        Self::new(repo, api_base, upload_base)
    }

    fn new(repo: &str, api_base: String, upload_base: String) -> Result<Self, PipelineError> {
        Ok(Self {
            client: pipeline_client()?,
            repo: repo.trim_matches('/').to_string(),
            token: read_token(Provider::GitHub)?,
            api_base,
            upload_base,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.api_base, path)
    }

    /// Bounded scan of the authenticated releases list (which includes the
    /// caller's draft releases) for a release carrying this tag. Three
    /// pages of 100 — far beyond this repository's release count — keep
    /// the scan bounded; the scan never paginates unboundedly.
    fn find_release_by_list_scan(&self, tag: &str) -> Result<Option<RemoteRelease>, PipelineError> {
        for page in 1..=3 {
            let url = self.url(&format!(
                "/repos/{}/releases?per_page=100&page={page}",
                self.repo
            ));
            let response = self
                .request(reqwest::Method::GET, &url)
                .send()
                .map_err(|e| PipelineError::Publish {
                    detail: format!("releases list scan failed: {e}"),
                })?;
            let status = response.status().as_u16();
            if status != 200 {
                return Err(PipelineError::Publish {
                    detail: format!("releases list scan returned HTTP {status}"),
                });
            }
            let values: Vec<serde_json::Value> =
                response.json().map_err(|e| PipelineError::Publish {
                    detail: format!("releases list is not JSON: {e}"),
                })?;
            for value in &values {
                if value.get("tag_name").and_then(|t| t.as_str()) == Some(tag) {
                    return parse_release(value, Provider::GitHub).map(Some);
                }
            }
            if values.len() < 100 {
                break;
            }
        }
        Ok(None)
    }

    fn request(&self, method: reqwest::Method, url: &str) -> reqwest::blocking::RequestBuilder {
        self.client
            .request(method, url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
    }
}

impl ProviderApi for GitHubPublisher {
    fn provider(&self) -> Provider {
        Provider::GitHub
    }

    fn is_production_endpoint(&self) -> bool {
        self.api_base == "https://api.github.com"
    }

    fn find_release(&self, tag: &str) -> Result<Option<RemoteRelease>, PipelineError> {
        let url = self.url(&format!("/repos/{}/releases/tags/{}", self.repo, tag));
        let response = self
            .request(reqwest::Method::GET, &url)
            .send()
            .map_err(|e| PipelineError::Publish {
                detail: format!("GET release by tag failed: {e}"),
            })?;
        match response.status().as_u16() {
            200 => {
                let value: serde_json::Value =
                    response.json().map_err(|e| PipelineError::Publish {
                        detail: format!("release response is not JSON: {e}"),
                    })?;
                Ok(Some(parse_release(&value, Provider::GitHub)?))
            }
            404 => {
                // Real GitHub shape (Phase 4A-1 smoke): a DRAFT release is
                // not resolvable by tag — the tag ref is created only when
                // the draft is published, so `releases/tags/{tag}` 404s
                // even though the draft exists. The authenticated releases
                // list does include the caller's drafts: fall back to a
                // bounded list scan.
                self.find_release_by_list_scan(tag)
            }
            status => Err(PipelineError::Publish {
                detail: format!("GET release by tag returned HTTP {status}"),
            }),
        }
    }

    fn create_release(
        &self,
        tag: &str,
        title: &str,
        body: &str,
        prerelease: bool,
        draft: bool,
    ) -> Result<RemoteRelease, PipelineError> {
        let url = self.url(&format!("/repos/{}/releases", self.repo));
        let payload = serde_json::json!({
            "tag_name": tag,
            "name": title,
            "body": body,
            "draft": draft,
            "prerelease": prerelease,
        });
        let response = self
            .request(reqwest::Method::POST, &url)
            .json(&payload)
            .send()
            .map_err(|e| PipelineError::Publish {
                detail: format!("create release failed: {e}"),
            })?;
        let status = response.status().as_u16();
        if status != 201 {
            let detail = response.text().unwrap_or_default();
            return Err(PipelineError::Publish {
                detail: format!("create release returned HTTP {status}: {detail}"),
            });
        }
        let value: serde_json::Value = response.json().map_err(|e| PipelineError::Publish {
            detail: format!("create release response is not JSON: {e}"),
        })?;
        parse_release(&value, Provider::GitHub)
    }

    fn upload_asset(
        &self,
        release: &RemoteRelease,
        name: &str,
        path: &Path,
    ) -> Result<(), PipelineError> {
        // GitHub issues a per-release upload URL template
        // `...{?name,label}`; strip the template suffix and add the name.
        let upload_base = release
            .upload_url
            .as_deref()
            .and_then(|template| template.split('{').next())
            .ok_or(PipelineError::Publish {
                detail: "release carries no upload_url".to_string(),
            })?
            .to_string();
        // The upload URL may come with its own host; only its path is reused
        // against the configured upload base (so rehearsal overrides hold).
        // The upload URL may be absolute (production) or path-only (mock /
        // rehearsal endpoints): keep the path component either way so the
        // configured upload base governs the host.
        let path_part = match upload_base.split_once("://") {
            Some((_, rest)) => rest
                .find('/')
                .map(|index| rest[index..].to_string())
                .ok_or(PipelineError::Publish {
                    detail: "upload_url has no path".to_string(),
                })?,
            None => {
                if upload_base.starts_with('/') {
                    upload_base
                } else {
                    return Err(PipelineError::Publish {
                        detail: "upload_url has no path".to_string(),
                    });
                }
            }
        };
        let url = format!("{}{}?name={}", self.upload_base, path_part, name);
        let file = std::fs::File::open(path).map_err(|e| PipelineError::Io {
            detail: format!("cannot open {} for upload: {e}", path.display()),
        })?;
        let size = file.metadata().map_err(|e| PipelineError::Io {
            detail: e.to_string(),
        })?.len();
        let response = self
            .request(reqwest::Method::POST, &url)
            .header("Content-Type", "application/octet-stream")
            .body(reqwest::blocking::Body::sized(file, size))
            .send()
            .map_err(|e| PipelineError::Publish {
                detail: format!("asset upload failed: {e}"),
            })?;
        let status = response.status().as_u16();
        if status != 201 {
            let detail = response.text().unwrap_or_default();
            return Err(PipelineError::Publish {
                detail: format!("asset upload returned HTTP {status}: {detail}"),
            });
        }
        Ok(())
    }

    fn set_publication(&self, release: &RemoteRelease, draft: bool) -> Result<(), PipelineError> {
        let url = self.url(&format!("/repos/{}/releases/{}", self.repo, release.id));
        let payload = serde_json::json!({ "draft": draft });
        let response = self
            .request(reqwest::Method::PATCH, &url)
            .json(&payload)
            .send()
            .map_err(|e| PipelineError::Publish {
                detail: format!("set publication failed: {e}"),
            })?;
        let status = response.status().as_u16();
        if status != 200 {
            let detail = response.text().unwrap_or_default();
            return Err(PipelineError::Publish {
                detail: format!("set publication returned HTTP {status}: {detail}"),
            });
        }
        Ok(())
    }

    fn download(&self, url: &str, writer: &mut dyn Write) -> Result<(), PipelineError> {
        let response = self
            .client
            .get(url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/octet-stream")
            .send()
            .map_err(|e| PipelineError::Publish {
                detail: format!("asset download failed: {e}"),
            })?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(PipelineError::Publish {
                detail: format!("asset download returned HTTP {status}"),
            });
        }
        copy_stream(response, writer)
    }
}

/// Stream a blocking response body into a writer.
fn copy_stream(
    mut response: reqwest::blocking::Response,
    writer: &mut dyn Write,
) -> Result<(), PipelineError> {
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let read = response.read(&mut chunk).map_err(|e| PipelineError::Publish {
            detail: format!("body read failed: {e}"),
        })?;
        if read == 0 {
            return Ok(());
        }
        writer
            .write_all(&chunk[..read])
            .map_err(|e| PipelineError::Io {
                detail: e.to_string(),
            })?;
    }
}

// ---------------------------------------------------------------- Gitee

/// Gitee v5 API publisher. Gitee has no draft state: a created release is
/// immediately visible, so the safe operator order is GitHub-draft →
/// read-back → Gitee, and the tool reports Gitee visibility honestly.
pub struct GiteePublisher {
    client: reqwest::blocking::Client,
    repo: String,
    token: String,
    api_base: String,
}

impl GiteePublisher {
    pub fn production(repo: &str) -> Result<Self, PipelineError> {
        Self::new(repo, "https://gitee.com/api/v5".to_string())
    }

    pub fn for_rehearsal(repo: &str, api_base: String) -> Result<Self, PipelineError> {
        Self::new(repo, api_base)
    }

    fn new(repo: &str, api_base: String) -> Result<Self, PipelineError> {
        Ok(Self {
            client: pipeline_client()?,
            repo: repo.trim_matches('/').to_string(),
            token: read_token(Provider::Gitee)?,
            api_base,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}?access_token={}", self.api_base, path, self.token)
    }

    /// The repository's default branch (real Gitee shape, Phase 4A-2:
    /// required as `target_commitish` when a release creates its own tag).
    fn default_branch(&self) -> Result<String, PipelineError> {
        let url = self.url(&format!("/repos/{}", self.repo));
        let response = self.client.get(&url).send().map_err(|e| PipelineError::Publish {
            detail: format!("GET repository failed: {e}"),
        })?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(PipelineError::Publish {
                detail: format!("GET repository returned HTTP {status}"),
            });
        }
        let value: serde_json::Value = response.json().map_err(|e| PipelineError::Publish {
            detail: format!("repository response is not JSON: {e}"),
        })?;
        if value.is_null() {
            return Err(PipelineError::Publish {
                detail: "repository response is null".to_string(),
            });
        }
        let branch = json_string(&value, "default_branch")?;
        if branch.is_empty() {
            return Err(PipelineError::Publish {
                detail: "repository response carries no default_branch".to_string(),
            });
        }
        Ok(branch)
    }
}

impl ProviderApi for GiteePublisher {
    fn provider(&self) -> Provider {
        Provider::Gitee
    }

    fn is_production_endpoint(&self) -> bool {
        self.api_base == "https://gitee.com/api/v5"
    }

    fn find_release(&self, tag: &str) -> Result<Option<RemoteRelease>, PipelineError> {
        let url = self.url(&format!("/repos/{}/releases/tags/{}", self.repo, tag));
        let response = self.client.get(&url).send().map_err(|e| PipelineError::Publish {
            detail: format!("GET release by tag failed: {e}"),
        })?;
        match response.status().as_u16() {
            200 => {
                let value: serde_json::Value =
                    response.json().map_err(|e| PipelineError::Publish {
                        detail: format!("release response is not JSON: {e}"),
                    })?;
                // Real Gitee shape (Phase 4A-2): a release lookup for a tag
                // with no release returns HTTP 200 with a literal `null`
                // body, not 404 — treat it as "no release exists".
                if value.is_null() {
                    return Ok(None);
                }
                Ok(Some(parse_release(&value, Provider::Gitee)?))
            }
            404 => Ok(None),
            status => Err(PipelineError::Publish {
                detail: format!("GET release by tag returned HTTP {status}"),
            }),
        }
    }

    fn create_release(
        &self,
        tag: &str,
        title: &str,
        body: &str,
        prerelease: bool,
        _draft: bool,
    ) -> Result<RemoteRelease, PipelineError> {
        // Gitee has no draft state; `_draft` is recorded as unsupported and
        // the caller's report must state the release is immediately visible.
        let url = self.url(&format!("/repos/{}/releases", self.repo));
        // Real Gitee shape (Phase 4A-2): creating a release for a tag that
        // does not exist yet requires `target_commitish` — the branch or
        // commit the tag is created on. Resolve the repository's actual
        // default branch (one authenticated read) instead of assuming it.
        let target_commitish = self.default_branch()?;
        let payload = serde_json::json!({
            "tag_name": tag,
            "name": title,
            "body": body,
            "prerelease": prerelease,
            "target_commitish": target_commitish,
        });
        let response = self.client.post(&url).json(&payload).send().map_err(|e| {
            PipelineError::Publish {
                detail: format!("create release failed: {e}"),
            }
        })?;
        let status = response.status().as_u16();
        if status != 201 {
            let detail = response.text().unwrap_or_default();
            return Err(PipelineError::Publish {
                detail: format!("create release returned HTTP {status}: {detail}"),
            });
        }
        let value: serde_json::Value = response.json().map_err(|e| PipelineError::Publish {
            detail: format!("create release response is not JSON: {e}"),
        })?;
        parse_release(&value, Provider::Gitee)
    }

    fn upload_asset(
        &self,
        release: &RemoteRelease,
        name: &str,
        path: &Path,
    ) -> Result<(), PipelineError> {
        // Gitee v5: POST /repos/{owner}/{repo}/releases/{id}/attach_files,
        // multipart/form-data with a `file` part. reqwest's multipart feature
        // is deliberately not enabled in this crate (the exact-bytes
        // transport rule pins the feature set), so the multipart body is
        // encoded by hand — a fixed boundary plus the file bytes.
        let url = self.url(&format!(
            "/repos/{}/releases/{}/attach_files",
            self.repo, release.id
        ));
        let bytes = std::fs::read(path).map_err(|e| PipelineError::Io {
            detail: format!("cannot read {} for upload: {e}", path.display()),
        })?;
        let boundary = "dtw-release-pipeline-attach-boundary";
        let mut body = Vec::with_capacity(bytes.len() + 256);
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(&bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let response = self
            .client
            .post(&url)
            .header(
                "Content-Type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(body)
            .send()
            .map_err(|e| PipelineError::Publish {
                detail: format!("asset upload failed: {e}"),
            })?;
        let status = response.status().as_u16();
        if status != 201 {
            let detail = response.text().unwrap_or_default();
            return Err(PipelineError::Publish {
                detail: format!("asset upload returned HTTP {status}: {detail}"),
            });
        }
        Ok(())
    }

    fn set_publication(&self, _release: &RemoteRelease, draft: bool) -> Result<(), PipelineError> {
        Err(PipelineError::Publish {
            detail: if draft {
                "Gitee has no draft state; releases are visible on creation".to_string()
            } else {
                "Gitee releases are published on creation; no finalize step exists".to_string()
            },
        })
    }

    fn download(&self, url: &str, writer: &mut dyn Write) -> Result<(), PipelineError> {
        let response = self.client.get(url).send().map_err(|e| PipelineError::Publish {
            detail: format!("asset download failed: {e}"),
        })?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(PipelineError::Publish {
                detail: format!("asset download returned HTTP {status}"),
            });
        }
        copy_stream(response, writer)
    }
}

// ---------------------------------------------------------------- engine

/// One publish decision (reported by dry-run and executed otherwise).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishDecision {
    /// Release does not exist yet and will be created (as draft where
    /// supported).
    CreateRelease { draft: bool },
    /// Asset missing remotely and will be uploaded.
    UploadAsset { name: String },
    /// Asset present remotely with identical bytes: no action, success.
    IdempotentAsset { name: String },
}

/// Plan the publish without mutating anything (read-only provider calls).
pub fn plan_publish(
    api: &dyn ProviderApi,
    artifacts: &ArtifactSet,
    tag: &str,
    mode: ReleaseMode,
) -> Result<(Option<RemoteRelease>, Vec<PublishDecision>), PipelineError> {
    let existing = api.find_release(tag)?;
    let mut decisions = Vec::new();
    if let Some(release) = &existing {
        enforce_rehearsal_visibility_isolation(api, release, mode)?;
        // Never point an existing tag elsewhere and never recreate; the
        // asset-level collision policy (read-only downloads included)
        // produces the exact per-asset decisions.
        decide_assets(api, release, artifacts, &mut decisions, false)?;
    } else {
        let draft = matches!(api.provider(), Provider::GitHub);
        decisions.push(PublishDecision::CreateRelease { draft });
        for (name, _) in artifacts.ordered() {
            decisions.push(PublishDecision::UploadAsset {
                name: name.to_string(),
            });
        }
    }
    Ok((existing, decisions))
}

/// Execute the publish with the full collision policy. Returns the
/// (possibly newly created) release record.
pub fn execute_publish(
    api: &dyn ProviderApi,
    artifacts: &ArtifactSet,
    tag: &str,
    title: &str,
    body: &str,
    mode: ReleaseMode,
) -> Result<(RemoteRelease, Vec<PublishDecision>), PipelineError> {
    let mut decisions = Vec::new();
    let release = match api.find_release(tag)? {
        Some(release) => {
            enforce_rehearsal_visibility_isolation(api, &release, mode)?;
            decide_assets(api, &release, artifacts, &mut decisions, true)?;
            release
        }
        None => {
            let draft = matches!(api.provider(), Provider::GitHub);
            let created = api.create_release(tag, title, body, mode == ReleaseMode::Rehearsal, draft)?;
            decisions.push(PublishDecision::CreateRelease { draft });
            decide_assets(api, &created, artifacts, &mut decisions, true)?;
            created
        }
    };
    Ok((release, decisions))
}

/// The per-release half of the rehearsal safety invariant: on a production
/// endpoint, rehearsal artifacts may only be attached to a release that
/// stable updater discovery will not select. GitHub isolation is the draft
/// state (invisible to discovery entirely); Gitee has no draft state, so its
/// isolation is the provider's prerelease publication flag — the exact field
/// the discovery adapters' publication-eligibility step filters on — chosen
/// together with an explicitly non-production version identity. A release
/// carrying rehearsal material that is publication-visible on its provider
/// is a safety violation, never an idempotence case.
pub(crate) fn enforce_rehearsal_visibility_isolation(
    api: &dyn ProviderApi,
    release: &RemoteRelease,
    mode: ReleaseMode,
) -> Result<(), PipelineError> {
    if mode == ReleaseMode::Rehearsal && api.is_production_endpoint() {
        let isolated = match api.provider() {
            Provider::GitHub => release.draft,
            Provider::Gitee => release.prerelease,
        };
        if !isolated {
            return Err(PipelineError::RehearsalSafety {
                detail: format!(
                    "release {} is publication-visible on the production endpoint; rehearsal \
                     artifacts may only touch a production endpoint through a DRAFT release \
                     (GitHub) or a prerelease-marked release (Gitee)",
                    release.tag
                ),
            });
        }
    }
    Ok(())
}

/// The exposure act (`--finalize`, draft → published) must never run for
/// rehearsal artifacts against a production endpoint: finalizing is the one
/// operation that makes a release visible to updater discovery.
pub fn enforce_finalize_safety(
    api: &dyn ProviderApi,
    mode: ReleaseMode,
) -> Result<(), PipelineError> {
    if mode == ReleaseMode::Rehearsal && api.is_production_endpoint() {
        return Err(PipelineError::RehearsalSafety {
            detail: "finalize would expose the release to production updater discovery; \
                     rehearsal releases must stay drafts"
                .to_string(),
        });
    }
    Ok(())
}

/// The asset-level collision policy, shared by plan and execute. With
/// `execute` the missing assets are uploaded (each immediately read back);
/// without it only decisions are recorded.
fn decide_assets(
    api: &dyn ProviderApi,
    release: &RemoteRelease,
    artifacts: &ArtifactSet,
    decisions: &mut Vec<PublishDecision>,
    execute: bool,
) -> Result<(), PipelineError> {
    for (name, path) in artifacts.ordered() {
        let remote_matches: Vec<&RemoteAsset> =
            release.assets.iter().filter(|asset| asset.name == name).collect();
        match remote_matches.as_slice() {
            [] => {
                decisions.push(PublishDecision::UploadAsset {
                    name: name.to_string(),
                });
                if execute {
                    api.upload_asset(release, &name, path)?;
                    verify_uploaded_asset(api, release, &name, path)?;
                }
            }
            [asset] => {
                // Idempotence check: download the remote asset and compare
                // bytes. Same bytes → no action; different bytes → fail
                // closed (covers partial uploads too).
                let local = std::fs::read(path).map_err(|e| PipelineError::Io {
                    detail: format!("cannot read local artifact {}: {e}", path.display()),
                })?;
                let mut remote_bytes = Vec::with_capacity(local.len());
                api.download(&asset.url, &mut remote_bytes)?;
                if remote_bytes != local {
                    return Err(PipelineError::PublishCollision {
                        detail: format!(
                            "remote asset {name:?} exists with different bytes; no silent \
                             overwrite of published signed artifacts (delete the remote asset \
                             explicitly if this is an intentional replacement)"
                        ),
                    });
                }
                decisions.push(PublishDecision::IdempotentAsset {
                    name: name.to_string(),
                });
            }
            _ => {
                return Err(PipelineError::PublishCollision {
                    detail: format!(
                        "provider lists {} assets named {name:?}; ambiguous remote state",
                        remote_matches.len()
                    ),
                });
            }
        }
    }
    Ok(())
}

/// After an upload, read the asset back immediately and compare digests:
/// the upload itself is only trusted once the bytes round-trip.
fn verify_uploaded_asset(
    api: &dyn ProviderApi,
    release: &RemoteRelease,
    name: &str,
    path: &Path,
) -> Result<(), PipelineError> {
    let fresh = api.find_release(&release.tag)?.ok_or(PipelineError::Publish {
        detail: "release disappeared immediately after asset upload".to_string(),
    })?;
    let asset = fresh
        .assets
        .iter()
        .filter(|asset| asset.name == name)
        .exactly_one()
        .map_err(|_| PipelineError::Publish {
            detail: format!("provider asset listing for {name:?} is ambiguous after upload"),
        })?;
    let local = std::fs::read(path).map_err(|e| PipelineError::Io {
        detail: e.to_string(),
    })?;
    let mut remote_bytes = Vec::with_capacity(local.len());
    api.download(&asset.url, &mut remote_bytes)?;
    if remote_bytes != local {
        return Err(PipelineError::ReadBack {
            detail: format!(
                "asset {name:?} does not round-trip: uploaded bytes differ from local bytes"
            ),
        });
    }
    Ok(())
}

/// Result of a full read-back verification of one provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadBackResult {
    pub provider: &'static str,
    /// SHA-256 of every verified remote artifact, by exact asset name.
    pub artifact_sha256: Vec<(String, String)>,
    /// SHA-256 of the signed manifest's exact remote bytes (the digest that
    /// binds the handoff/consent chain).
    pub manifest_sha256_hex: String,
    /// The envelope key id observed remotely.
    pub key_id: String,
}

/// Full read-back verification (§15): download the exact remote artifacts to
/// an isolated temp directory, compare bytes to the local prepared
/// artifacts, verify the envelope + manifest under the requested mode's
/// trust store, verify the manifest's package facts, validate the package
/// ZIP through the shared validator, and verify the two managed EXEs.
///
/// The remote tag is passed explicitly: the frozen convention is
/// `v{version}` (`tag_for_version`), but a rehearsal release publishes
/// under an explicit non-production tag, and read-back must verify the
/// release the publisher actually created — never a re-derived one.
pub fn read_back(
    api: &dyn ProviderApi,
    artifacts: &ArtifactSet,
    facts: &ReleaseFacts,
    trust: &desktop_todo_update_core::TrustStore,
    staging: &Path,
    tag: &str,
) -> Result<ReadBackResult, PipelineError> {
    let release = api
        .find_release(tag)?
        .ok_or(PipelineError::ReadBack {
            detail: "no remote release exists for the tag".to_string(),
        })?;
    enforce_rehearsal_visibility_isolation(api, &release, facts.mode)?;

    let temp = staging.join("read-back-temp");
    std::fs::create_dir_all(&temp).map_err(|e| PipelineError::Io {
        detail: e.to_string(),
    })?;
    let cleanup = |temp: &Path| {
        let _ = std::fs::remove_dir_all(temp);
    };

    let mut artifact_sha256: Vec<(String, String)> = Vec::new();
    for (name, path) in artifacts.ordered() {
        let remote = release
            .assets
            .iter()
            .filter(|asset| asset.name == name)
            .exactly_one()
            .map_err(|_| PipelineError::ReadBack {
                detail: format!(
                    "remote release does not carry exactly one asset named {name:?}"
                ),
            })?;
        // Download the exact remote bytes into the isolated temp directory
        // first; every comparison happens against the downloaded file, never
        // against provider metadata.
        let downloaded_path = temp.join(format!("read-back-{}", sanitize_name(&name)));
        {
            let mut file = std::fs::File::create(&downloaded_path).map_err(|e| {
                cleanup(&temp);
                PipelineError::Io {
                    detail: e.to_string(),
                }
            })?;
            api.download(&remote.url, &mut file).map_err(|e| {
                cleanup(&temp);
                e
            })?;
            file.sync_all().map_err(|e| {
                cleanup(&temp);
                PipelineError::Io {
                    detail: e.to_string(),
                }
            })?;
        }
        let downloaded = std::fs::read(&downloaded_path).map_err(|e| {
            cleanup(&temp);
            PipelineError::Io {
                detail: e.to_string(),
            }
        })?;
        let local = std::fs::read(path).map_err(|e| {
            cleanup(&temp);
            PipelineError::Io {
                detail: format!("cannot read local artifact {}: {e}", path.display()),
            }
        })?;
        if downloaded != local {
            cleanup(&temp);
            return Err(PipelineError::ReadBack {
                detail: format!("remote asset {name:?} bytes differ from the local prepared artifact"),
            });
        }
        artifact_sha256.push((
            name,
            desktop_todo_update_core::sha256_hex(&downloaded),
        ));
    }

    let manifest_bytes = std::fs::read(&artifacts.manifest).map_err(|e| PipelineError::Io {
        detail: e.to_string(),
    })?;
    let envelope_bytes = std::fs::read(&artifacts.envelope).map_err(|e| PipelineError::Io {
        detail: e.to_string(),
    })?;

    // Envelope + manifest verification under the requested mode's trust
    // store (production: the actual compiled production store; rehearsal:
    // the explicit test store — never impersonating production).
    let verified = desktop_todo_release_signer::verify_envelope(trust, &envelope_bytes, &manifest_bytes)
        .map_err(|e| {
            cleanup(&temp);
            PipelineError::ReadBack {
                detail: format!("remote manifest/envelope failed verification: {e}"),
            }
        })?;

    // Manifest package facts must match the downloaded package bytes.
    let package_bytes =
        std::fs::read(&artifacts.package).map_err(|e| PipelineError::Io {
            detail: e.to_string(),
        })?;
    if package_bytes.len() as u64 != verified.package_size
        || desktop_todo_update_core::sha256_hex(&package_bytes) != verified.package_sha256
    {
        cleanup(&temp);
        return Err(PipelineError::ReadBack {
            detail: "verified manifest package facts disagree with the downloaded package bytes"
                .to_string(),
        });
    }

    // Package ZIP validation through the shared validator, and the two
    // managed EXEs against the release facts (which the manifest was
    // generated from and the signer self-verified).
    {
        let file = std::fs::File::open(&artifacts.package).map_err(|e| PipelineError::Io {
            detail: e.to_string(),
        })?;
        let mut archive =
            desktop_todo_maintenance::package_zip::validate_archive(file).map_err(|e| {
                cleanup(&temp);
                PipelineError::ReadBack {
                    detail: format!("remote package failed the shared ZIP validation: {e:?}"),
                }
            })?;
        if archive.len() != 9 {
            cleanup(&temp);
            return Err(PipelineError::ReadBack {
                detail: "remote package does not carry exactly the nine allowlist entries"
                    .to_string(),
            });
        }
        for entry in &facts.install_files {
            let (size, sha) = crate::assemble::validated_entry_facts(&mut archive, &entry.filename)?;
            if size != entry.size || sha != entry.sha256_hex {
                cleanup(&temp);
                return Err(PipelineError::ReadBack {
                    detail: format!(
                        "packaged EXE {} disagrees with the release facts",
                        entry.filename
                    ),
                });
            }
        }
    }

    cleanup(&temp);
    Ok(ReadBackResult {
        provider: api.provider().as_str(),
        artifact_sha256,
        manifest_sha256_hex: verified.manifest_sha256,
        key_id: verified.key_id,
    })
}

/// Mirror consistency (§14): the two providers must agree on every
/// artifact's exact SHA-256 — never only on metadata names.
pub fn assert_mirror_consistency(
    left: &ReadBackResult,
    right: &ReadBackResult,
) -> Result<(), PipelineError> {
    if left.artifact_sha256.len() != right.artifact_sha256.len() {
        return Err(PipelineError::MirrorDisagreement {
            detail: format!(
                "artifact counts differ: {} vs {}",
                left.artifact_sha256.len(),
                right.artifact_sha256.len()
            ),
        });
    }
    for (name, sha) in &left.artifact_sha256 {
        let Some((_, other_sha)) = right
            .artifact_sha256
            .iter()
            .find(|(other_name, _)| other_name == name)
        else {
            return Err(PipelineError::MirrorDisagreement {
                detail: format!("artifact {name:?} missing from the second provider"),
            });
        };
        if other_sha != sha {
            return Err(PipelineError::MirrorDisagreement {
                detail: format!(
                    "artifact {name:?} bytes differ between providers ({sha} vs {other_sha})"
                ),
            });
        }
    }
    if left.manifest_sha256_hex != right.manifest_sha256_hex {
        return Err(PipelineError::MirrorDisagreement {
            detail: "manifest digest differs between providers".to_string(),
        });
    }
    if left.key_id != right.key_id {
        return Err(PipelineError::MirrorDisagreement {
            detail: "envelope key id differs between providers".to_string(),
        });
    }
    Ok(())
}

/// The frozen tag convention: `v` + the manifest's three-part stable
/// version (repo tag convention; never a new grammar).
pub fn tag_for_version(version: &str) -> Result<String, PipelineError> {
    let parsed = desktop_todo_update_core::Version::parse(version).map_err(|e| {
        PipelineError::ReleaseIdentity {
            detail: format!("version {version:?} is not a valid protocol-1 target: {e}"),
        }
    })?;
    Ok(format!("v{parsed}"))
}

/// A tiny extension mirroring `Iterator::exactly_one` (itertools is not a
/// dependency): Ok(item) for exactly one, Err otherwise.
trait ExactlyOne: Iterator + Sized {
    fn exactly_one(self) -> Result<Self::Item, ()> {
        let mut iter = self;
        let first = iter.next().ok_or(())?;
        if iter.next().is_some() {
            return Err(());
        }
        Ok(first)
    }
}
impl<I: Iterator> ExactlyOne for I {}

/// Reduce a remote asset name to a safe local temp filename component: the
/// asset names are provider-controlled, so never pass them through to a
/// path join unfiltered. Only alphanumerics, `.`, `_`, and `-` survive; any
/// other character becomes `_`.
fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

