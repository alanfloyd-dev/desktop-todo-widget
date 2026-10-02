//! Provider-neutral candidate model plus the GitHub and Gitee discovery
//! adapters.
//!
//! Frozen boundary: the providers answer only "where is a candidate
//! release?". Everything they return — ids, tags, timestamps, URLs — is
//! attacker-controlled discovery metadata with no authority; release titles,
//! tags, and ordering never override a signed manifest field. Provider
//! identity never participates in trust, so both adapters normalize into the
//! same [`CandidateDescriptor`] and there is exactly one verifier, one
//! version comparator, and one eligibility policy (the shared update-core).

use serde::Deserialize;

use crate::updater::http_fetch::{fetch_bounded, BodyKind, FetchError};
use crate::updater::{with_bounded_retry, DiscoveryConfig};

/// Which provider produced a descriptor. Diagnostics and transport routing
/// only — it can never widen trust, and the two providers meet the identical
/// signature requirements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    GitHub,
    Gitee,
}

/// The frozen metadata asset names, siblings of the release ZIP (protocol v1:
/// the manifest and signature stay outside the ZIP).
pub const MANIFEST_ASSET_NAME: &str = "update-manifest.json";
pub const ENVELOPE_ASSET_NAME: &str = "update-manifest.json.sig";

/// One bounded, provider-neutral candidate descriptor. It makes no claim —
/// never "verified", "trusted", "eligible", or "safe" — it only names where
/// the raw metadata bytes can be fetched.
#[derive(Debug, Clone)]
pub struct CandidateDescriptor {
    pub kind: ProviderKind,
    /// Provider-local release identifier (diagnostics only).
    pub release_id: String,
    /// Provider tag: discovery metadata with no authority.
    pub tag: String,
    /// Provider publication timestamp: discovery metadata with no authority.
    pub published_at: Option<String>,
    /// The metadata asset URLs, or the candidate-local reason they are
    /// unusable. Per the frozen candidate-local rule, an unusable release
    /// rejects only itself: the bounded enumeration continues to older
    /// candidates, so provider garbage can never block a still-valid,
    /// correctly signed bridge.
    pub metadata: Result<MetadataUrls, MetadataSkip>,
}

/// Upper bound on provider-listed assets carried per candidate
/// (implementation transport policy; the releases index itself is already
/// body-capped before parsing).
pub const MAX_ASSETS_PER_CANDIDATE: usize = 64;

#[derive(Debug, Clone)]
pub struct MetadataUrls {
    pub envelope: String,
    pub manifest: String,
    /// The candidate's provider asset list (name, URL), carried unchanged.
    /// No extension, suffix, or naming-grammar filter is applied here: the
    /// package locator is selected by exact match against the **signed
    /// manifest's package filename** after verification — the asset name is
    /// an opaque identifier, and every fetched byte is hash-verified against
    /// the signed package facts.
    pub assets: Vec<(String, String)>,
}

/// Candidate-local, content-level reasons a release cannot even be fetched
/// for verification. Never a transport failure and never a cryptographic
/// verdict — the scan continues past it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataSkip {
    /// The provider published the release without both frozen metadata
    /// assets.
    AssetsMissing,
    /// An asset URL is outside the adapter origin allowlist or not https.
    UnusableAssetUrl { detail: String },
}

/// Compiled endpoints for one provider. Repository addresses are pinned at
/// implementation time (protocol v1: "pinned and reviewed at implementation
/// time"); the values match the project's published GitHub/Gitee remotes and
/// must be re-reviewed at release time.
#[derive(Debug, Clone)]
pub struct ProviderEndpoints {
    pub kind: ProviderKind,
    /// The official releases endpoint; the `{bound}` placeholder is
    /// substituted with the compiled candidate bound. No other URL is ever
    /// used for enumeration.
    pub releases_url: String,
    /// Exact origins (`scheme://host[:port]`) a release asset or redirect may
    /// live on ("redirects and download hosts need adapter allowlists").
    pub download_origin_allowlist: Vec<String>,
}

pub const GITHUB_DOWNLOAD_ORIGINS: &[&str] = &[
    "https://github.com",
    // GitHub serves release assets through 302 redirects onto its S3-backed
    // CDN and does not pin a fixed CDN host; both documented hosts are
    // allowlisted and every other origin still fails closed.
    // objects.githubusercontent.com (legacy) and
    // release-assets.githubusercontent.com (documented current host).
    "https://objects.githubusercontent.com",
    "https://release-assets.githubusercontent.com",
];
pub const GITEE_DOWNLOAD_ORIGINS: &[&str] = &[
    "https://gitee.com",
    // Observed live (Phase 4A-2 rehearsal smoke, 2026-10-02): Gitee serves
    // release assets through a two-hop redirect chain,
    // gitee.com/.../releases/download/... → gitee.com/attach_files/<id>/
    // download/... → https://foruda.gitee.com/attach_file/... (signed CDN
    // URL) → 200. foruda.gitee.com is Gitee's own attachment CDN host on
    // the same registrable domain; it is admitted as an exact origin —
    // never a wildcard, never a suffix rule — so every other host
    // (including sibling subdomains and look-alike domains) still fails
    // closed, and the chain remains bounded by the shared redirect-count
    // limit and verified by the byte-exact digest rules downstream.
    "https://foruda.gitee.com",
];

/// The compiled production endpoints, GitHub first (Auto's frozen primary).
pub fn production_endpoints() -> Vec<ProviderEndpoints> {
    vec![
        ProviderEndpoints {
            kind: ProviderKind::GitHub,
            releases_url:
                "https://api.github.com/repos/alanfloyd-dev/desktop-todo-widget/releases?per_page={bound}"
                    .to_string(),
            download_origin_allowlist: GITHUB_DOWNLOAD_ORIGINS
                .iter()
                .map(|s| s.to_string())
                .collect(),
        },
        ProviderEndpoints {
            kind: ProviderKind::Gitee,
            releases_url:
                "https://gitee.com/api/v5/repos/alanfloyd-dev/desktop-todo-widget/releases?page=1&per_page={bound}"
                    .to_string(),
            download_origin_allowlist: GITEE_DOWNLOAD_ORIGINS
                .iter()
                .map(|s| s.to_string())
                .collect(),
        },
    ]
}

/// Every origin the shared discovery client may ever talk to or follow a
/// redirect to — the union of the providers' compiled allowlists.
pub fn combined_origin_allowlist(endpoints: &[ProviderEndpoints]) -> Vec<String> {
    let mut origins = Vec::new();
    for endpoint in endpoints {
        origins.extend(endpoint.download_origin_allowlist.iter().cloned());
    }
    origins
}

/// Provider release-list response model. Provider JSON is untrusted
/// transport data, not a protocol document: fields are read leniently and
/// the whole body is bounded before parsing, so unknown fields and shapes
/// cannot widen anything.
#[derive(Debug, Deserialize)]
struct ProviderRelease {
    id: u64,
    #[serde(default)]
    tag_name: String,
    /// Adapter-recognized publication handling: a draft is not a published
    /// release and is never enumerated.
    #[serde(default)]
    draft: bool,
    /// Protocol 1 delivers stable versions only; a provider-marked
    /// prerelease is not publication-eligible for the stable channel. The
    /// flag is never used to interpret a version — the signed manifest
    /// version remains the only security fact.
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<ProviderAsset>,
}

#[derive(Debug, Deserialize)]
struct ProviderAsset {
    #[serde(default)]
    name: String,
    #[serde(default)]
    browser_download_url: String,
}

/// Enumeration result: the bounded newest→oldest candidate set plus the
/// counts of releases dropped at the publication-eligibility step.
#[derive(Debug)]
pub struct Enumeration {
    pub candidates: Vec<CandidateDescriptor>,
    pub skipped_draft: usize,
    pub skipped_prerelease: usize,
}

/// Enumerate the bounded candidate set for one provider, newest→oldest.
///
/// Ordering authority: the frozen contract requires newest→oldest bounded
/// enumeration and forbids relying on a host's arbitrary "latest" shortcut,
/// so the adapter enumerates the bounded releases list in the provider's own
/// newest-first order. That order is discovery metadata only — scan order,
/// never a trust decision — and no reordering happens anywhere else.
pub fn enumerate_candidates(
    client: &reqwest::blocking::Client,
    endpoints: &ProviderEndpoints,
    config: &DiscoveryConfig,
    deadline: Option<std::time::Instant>,
) -> Result<Enumeration, FetchError> {
    let url = endpoints
        .releases_url
        .replace("{bound}", &config.max_candidates.to_string());
    let body = with_bounded_retry(config, deadline, |per_request_timeout| {
        fetch_bounded(
            client,
            &url,
            config.provider_index_body_cap,
            BodyKind::ProviderIndex,
            per_request_timeout,
        )
    })?;
    let releases: Vec<ProviderRelease> =
        serde_json::from_slice(&body).map_err(|e| FetchError::ResponseMalformed {
            detail: format!("provider releases response is not the expected JSON shape: {e}"),
        })?;

    let mut candidates = Vec::new();
    let mut skipped_draft = 0;
    let mut skipped_prerelease = 0;
    for release in releases.into_iter().take(config.max_candidates) {
        if release.draft {
            skipped_draft += 1;
            continue;
        }
        if release.prerelease {
            skipped_prerelease += 1;
            continue;
        }
        let metadata = metadata_urls(&release, endpoints);
        candidates.push(CandidateDescriptor {
            kind: endpoints.kind,
            release_id: release.id.to_string(),
            tag: release.tag_name,
            published_at: release.published_at,
            metadata,
        });
    }
    Ok(Enumeration {
        candidates,
        skipped_draft,
        skipped_prerelease,
    })
}

fn metadata_urls(
    release: &ProviderRelease,
    endpoints: &ProviderEndpoints,
) -> Result<MetadataUrls, MetadataSkip> {
    let find = |name: &str| {
        release
            .assets
            .iter()
            .find(|asset| asset.name == name)
            .map(|asset| asset.browser_download_url.clone())
    };
    let Some(envelope) = find(ENVELOPE_ASSET_NAME) else {
        return Err(MetadataSkip::AssetsMissing);
    };
    let Some(manifest) = find(MANIFEST_ASSET_NAME) else {
        return Err(MetadataSkip::AssetsMissing);
    };
    for url in [&envelope, &manifest] {
        if !asset_url_allowed(url, endpoints) {
            return Err(MetadataSkip::UnusableAssetUrl {
                detail: format!("asset url {url:?} is outside the adapter origin allowlist"),
            });
        }
    }
    let assets: Vec<(String, String)> = release
        .assets
        .iter()
        .take(MAX_ASSETS_PER_CANDIDATE)
        .map(|asset| (asset.name.clone(), asset.browser_download_url.clone()))
        .collect();
    Ok(MetadataUrls {
        envelope,
        manifest,
        assets,
    })
}

/// An asset URL must land on one of the provider's compiled download
/// origins — scheme included, so the https-only production allowlist also
/// enforces the scheme ("download hosts need adapter allowlists"). The
/// provider can hand us any URL, and the updater is not a generic URL
/// fetcher.
fn asset_url_allowed(url: &str, endpoints: &ProviderEndpoints) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    let (Some(scheme), Some(host)) = (Some(parsed.scheme()), parsed.host_str()) else {
        return false;
    };
    let port = match parsed.port() {
        Some(port) => format!(":{port}"),
        None => String::new(),
    };
    let origin = format!("{scheme}://{host}{port}").to_ascii_lowercase();
    endpoints
        .download_origin_allowlist
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(&origin))
}
