use crate::{
    database::GithubCacheRow,
    settings::AppState,
};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::Manager;
use url::Url;

/// The one host every GitHub REST request may touch. Usernames are validated
/// before URL construction, the request URLs are built from this fixed
/// origin only, and pagination targets are re-checked against it — a Link
/// header pointing anywhere else is corruption, not a page to fetch.
const API_ORIGIN: &str = "https://api.github.com";
/// The public profile page the dashboard's safe link opens. Validated
/// username + fixed origin, the same shape rule as the API origin above.
const PROFILE_ORIGIN: &str = "https://github.com";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Cache freshness (ADR-DG-001 ⑤ / G1 contract): a snapshot younger than this
/// is `fresh`; older is `stale` and keeps rendering with an explicit age.
const FRESH_AFTER_SECONDS: i64 = 30 * 60;
/// Manual refresh cooldown, the weather module's established shape.
const MANUAL_REFRESH_COOLDOWN: Duration = Duration::from_secs(30);
/// Response size cap. A full 100-repository page is far below this; anything
/// larger is refused instead of read, so no response can grow unbounded.
const MAX_BODY_BYTES: u64 = 4 * 1024 * 1024;
/// The bounded request budget: one profile request plus at most ten
/// repository pages (1000 owned repositories) per refresh. Pagination beyond
/// the budget fails the refresh as [`GithubError::Incomplete`] rather than
/// presenting a truncated sum as a total.
const MAX_REPOSITORY_PAGES: usize = 10;
/// How long a missing Retry-After is believed (GitHub's unauthenticated
/// window is hourly; ten minutes is the cautious default retry distance).
const DEFAULT_RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(10 * 60);
/// Upper bound on any rate-limit backoff this build will honor, so a hostile
/// Retry-After cannot turn one response into a day-long lockout.
const RATE_LIMIT_BACKOFF_CAP: Duration = Duration::from_secs(60 * 60);

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubSnapshot {
    /// The account's canonical login as the profile response names it. The
    /// cache key remains the configured username lowercased (GitHub usernames
    /// are case-insensitive); this field is what the integrity filter below
    /// checks the row against and what the UI can show verbatim.
    pub username: String,
    pub followers: u64,
    /// Total stars across the account's owned public non-fork repositories,
    /// archived repositories included. Only a COMPLETE traversal is ever
    /// stored: a pagination budget that ran out produces an error and keeps
    /// the previous snapshot, never a partial sum.
    pub stars: u64,
    pub fetched_at: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GithubCacheStatus {
    Fresh,
    Stale,
    Missing,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubViewState {
    /// Whether a username is configured at all. False renders the setup
    /// state; unconfigured is the first of the two zero-request gates.
    pub configured: bool,
    /// The configured username, as the user typed it (trimmed). Empty when
    /// unconfigured.
    pub username: String,
    pub snapshot: Option<GithubSnapshot>,
    pub cache_status: GithubCacheStatus,
    pub cache_age_seconds: Option<i64>,
    pub refreshing: bool,
    /// Whether a provider rate-limit backoff is currently in force; refresh
    /// attempts are refused without touching the network until it expires.
    pub rate_limited: bool,
    /// Remaining backoff seconds when [`Self::rate_limited`], for display.
    pub retry_after_seconds: Option<i64>,
    pub last_refresh: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "message", rename_all = "PascalCase")]
pub enum GithubError {
    /// No username configured (or the empty value). Refused before the
    /// refresh gate: the second zero-request gate, so the unconfigured module
    /// can never reach the network.
    NotConfigured,
    /// A configured value that cannot be a GitHub username. Refused before
    /// the refresh gate and before any URL is built from it.
    InvalidUsername,
    /// The profile request answered 404: no such public account.
    NotFound,
    /// The provider reported the rate budget exhausted (429, or 403 with
    /// `X-RateLimit-Remaining: 0`). Carries the provider's retry hint in
    /// seconds when it offered one; the runtime caps whatever it accepts.
    RateLimited(Option<i64>),
    /// The repository traversal hit the page budget before the provider
    /// reported an end. Never persisted: the previous complete snapshot
    /// survives instead.
    Incomplete,
    Network,
    Timeout,
    InvalidResponse,
    ProviderError,
    InProgress,
    Cooldown,
}

impl GithubError {
    fn diagnostic_label(&self) -> &'static str {
        match self {
            Self::NotConfigured => "not-configured",
            Self::InvalidUsername => "invalid-username",
            Self::NotFound => "not-found",
            Self::RateLimited(_) => "rate-limited",
            Self::Incomplete => "incomplete",
            Self::Network => "network-error",
            Self::Timeout => "timeout",
            Self::InvalidResponse => "invalid-response",
            Self::ProviderError => "provider-error",
            Self::InProgress => "in-progress",
            Self::Cooldown => "cooldown",
        }
    }
}

#[derive(Default)]
pub struct GithubRuntime {
    refreshing: AtomicBool,
    last_manual_refresh: Mutex<Option<Instant>>,
    rate_limited_until: Mutex<Option<Instant>>,
    last_refresh: Mutex<String>,
}

impl GithubRuntime {
    fn last_refresh(&self) -> String {
        self.last_refresh
            .lock()
            .map(|value| {
                if value.is_empty() {
                    "none".into()
                } else {
                    value.clone()
                }
            })
            .unwrap_or_else(|_| "unavailable".into())
    }

    fn record(&self, value: &str) {
        if let Ok(mut status) = self.last_refresh.lock() {
            *status = value.into();
        }
    }

    /// Remaining rate-limit backoff in seconds, when one is in force.
    fn rate_limit_remaining(&self) -> Option<i64> {
        let until = *self.rate_limited_until.lock().ok()?;
        let remaining = until?.checked_duration_since(Instant::now())?;
        Some(remaining.as_secs().max(1) as i64)
    }

    fn note_rate_limit(&self, retry_after_seconds: Option<i64>) {
        let duration = retry_after_seconds
            .and_then(|seconds| u64::try_from(seconds.max(1)).ok())
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_RATE_LIMIT_BACKOFF)
            .min(RATE_LIMIT_BACKOFF_CAP);
        if let Ok(mut until) = self.rate_limited_until.lock() {
            *until = Some(Instant::now() + duration);
        }
    }

    fn begin_refresh(&self, manual: bool) -> Result<(), GithubError> {
        // The rate-limit gate comes first: a backoff in force refuses every
        // refresh without any network activity, manual or not.
        if let Some(remaining) = self.rate_limit_remaining() {
            return Err(GithubError::RateLimited(Some(remaining)));
        }
        if manual {
            let mut last = self
                .last_manual_refresh
                .lock()
                .map_err(|_| GithubError::ProviderError)?;
            if last.is_some_and(|instant| instant.elapsed() < MANUAL_REFRESH_COOLDOWN) {
                return Err(GithubError::Cooldown);
            }
            *last = Some(Instant::now());
        }
        // Single-flight: a second refresh while one is in flight is refused,
        // which is also the backend half of concurrent-fetch deduplication —
        // a fetch always targets the currently configured username, so two
        // concurrent calls are by definition the same fetch.
        if self.refreshing.swap(true, Ordering::AcqRel) {
            return Err(GithubError::InProgress);
        }
        Ok(())
    }

    fn finish_refresh(&self) {
        self.refreshing.store(false, Ordering::Release);
    }
}

/// The settings-patch validator: an empty value is the explicit unconfigured
/// state and is storable; anything non-empty must be a GitHub username.
pub fn validate_username(raw: &str) -> Result<(), String> {
    let value = raw.trim();
    if value.is_empty() {
        return Ok(());
    }
    username_shape_error(value).map_or(Ok(()), |reason| Err(reason.to_string()))
}

/// The fetch-path validator: normalizes (trim) and refuses both the empty
/// value ([`GithubError::NotConfigured`]) and malformed usernames
/// ([`GithubError::InvalidUsername`]) before the refresh gate runs.
fn normalize_username(raw: &str) -> Result<String, GithubError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(GithubError::NotConfigured);
    }
    username_shape_error(value).map_or(Ok(value.to_string()), |_| Err(GithubError::InvalidUsername))
}

/// GitHub's documented username grammar: 1–39 characters, alphanumerics and
/// hyphens only, no leading or trailing hyphen. The check runs before any
/// URL construction, so a refused value can never reach a request.
fn username_shape_error(value: &str) -> Option<&'static str> {
    if value.chars().count() > 39 {
        return Some("longer than 39 characters");
    }
    if value.starts_with('-') {
        return Some("must not start with a hyphen");
    }
    if value.ends_with('-') {
        return Some("must not end with a hyphen");
    }
    if !value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Some("may only contain letters, digits, and hyphens");
    }
    None
}

/// The cache key for a validated username: lowercase, because GitHub
/// usernames are case-insensitive and one account has exactly one row.
fn cache_key(username: &str) -> String {
    username.to_lowercase()
}

struct HttpResponse {
    status: u16,
    retry_after: Option<String>,
    rate_limit_remaining: Option<String>,
    rate_limit_reset: Option<String>,
    link: Option<String>,
    body: String,
}

trait HttpTransport {
    fn get(&self, url: Url) -> Result<HttpResponse, GithubError>;
}

struct ReqwestTransport {
    client: reqwest::blocking::Client,
}

impl ReqwestTransport {
    fn new() -> Result<Self, GithubError> {
        reqwest::blocking::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            // Public HTTP identity, the same string the weather module sends.
            .user_agent(concat!("desktop-todo-widget/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                // Fixed-host rule: redirects are followed only while they stay
                // on the API origin. Anything else — a different host, a
                // scheme downgrade reqwest already refuses — stops here, and
                // the non-2xx answer then fails the request in the caller.
                if attempt.previous().len() > 4 {
                    attempt.stop()
                } else if attempt.url().host_str() == Some("api.github.com") {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .build()
            .map(|client| Self { client })
            .map_err(|_| GithubError::Network)
    }
}

impl HttpTransport for ReqwestTransport {
    fn get(&self, url: Url) -> Result<HttpResponse, GithubError> {
        let response = self
            .client
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .send()
            .map_err(|error| {
                if error.is_timeout() {
                    GithubError::Timeout
                } else {
                    GithubError::Network
                }
            })?;
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        };
        let retry_after = header("Retry-After");
        let rate_limit_remaining = header("X-RateLimit-Remaining");
        let rate_limit_reset = header("X-RateLimit-Reset");
        let link = header("Link");
        Ok(HttpResponse {
            status,
            retry_after,
            rate_limit_remaining,
            rate_limit_reset,
            link,
            body: bounded_body(response)?,
        })
    }
}

/// Reads a response body under the size cap: a longer body is an
/// [`GithubError::InvalidResponse`], never a truncated read presented as
/// data.
fn bounded_body(mut response: reqwest::blocking::Response) -> Result<String, GithubError> {
    let mut bytes = Vec::new();
    response
        .by_ref()
        .take(MAX_BODY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| GithubError::Network)?;
    if bytes.len() as u64 > MAX_BODY_BYTES {
        return Err(GithubError::InvalidResponse);
    }
    String::from_utf8(bytes).map_err(|_| GithubError::InvalidResponse)
}

#[derive(Deserialize)]
struct RawUser {
    login: String,
    followers: u64,
}

#[derive(Deserialize)]
struct RawRepository {
    fork: bool,
    #[serde(default)]
    private: bool,
    // Archived repositories carry no special handling: they are the account's
    // own work and are included unconditionally, so the flag is never read.
    stargazers_count: u64,
}

/// Builds a request URL under the fixed API origin. `segments` are appended
/// path segments; `query` pairs are appended in order (an empty query leaves
/// the URL without a query part). Panics are impossible: both mutators
/// report their (structural) failures as errors instead.
fn api_url(segments: &[&str], query: &[(&str, &str)]) -> Result<Url, GithubError> {
    let mut url = Url::parse(API_ORIGIN).map_err(|_| GithubError::ProviderError)?;
    {
        let mut builder = url.path_segments_mut().map_err(|_| GithubError::ProviderError)?;
        builder.pop_if_empty();
        for segment in segments {
            builder.push(segment);
        }
    }
    if !query.is_empty() {
        let mut pairs = url.query_pairs_mut();
        for (name, value) in query {
            pairs.append_pair(name, value);
        }
    }
    Ok(url)
}

fn profile_url(username: &str) -> Result<Url, GithubError> {
    api_url(&["users", username], &[])
}

fn repositories_url(username: &str) -> Result<Url, GithubError> {
    api_url(
        &["users", username, "repos"],
        &[("type", "owner"), ("per_page", "100")],
    )
}

/// Extracts the `rel="next"` pagination target from a Link header, and only
/// a target on the fixed API origin over HTTPS. No Link header, or a Link
/// header without a next relation, means the traversal ended.
fn next_page_url(response: &HttpResponse) -> Result<Option<Url>, GithubError> {
    let Some(link) = &response.link else {
        return Ok(None);
    };
    for segment in link.split(',') {
        let mut parts = segment.splitn(2, ';');
        let target = parts.next().unwrap_or("").trim();
        let params = parts.next().unwrap_or("");
        let is_next = params.split(';').any(|param| {
            param
                .trim()
                .strip_prefix("rel=")
                .is_some_and(|value| value.trim_matches('"').eq_ignore_ascii_case("next"))
        });
        if is_next {
            let url_text = target.strip_prefix('<').and_then(|rest| rest.strip_suffix('>'));
            let url = Url::parse(url_text.unwrap_or("")).map_err(|_| GithubError::InvalidResponse)?;
            if url.scheme() != "https" || url.host_str() != Some("api.github.com") {
                return Err(GithubError::InvalidResponse);
            }
            return Ok(Some(url));
        }
    }
    Ok(None)
}

/// Maps a refused profile/page status to the pinned error vocabulary:
/// rate-limit shapes are recognized exactly (429, or 403 with the remaining
/// budget at zero); everything else non-2xx is the generic provider error.
fn refused_status_error(response: &HttpResponse, now: i64) -> GithubError {
    let remaining_zero = response.rate_limit_remaining.as_deref() == Some("0");
    if response.status == 429 || (response.status == 403 && remaining_zero) {
        GithubError::RateLimited(retry_after_seconds(response, now))
    } else {
        GithubError::ProviderError
    }
}

/// The provider's retry hint, from `Retry-After` (integer seconds form) or
/// `X-RateLimit-Reset` (unix epoch seconds), in that order. Neither present
/// means `None`, and the runtime applies its own bounded default backoff.
fn retry_after_seconds(response: &HttpResponse, now: i64) -> Option<i64> {
    if let Some(seconds) = response
        .retry_after
        .as_deref()
        .and_then(|value| value.trim().parse::<i64>().ok())
    {
        return Some(seconds.max(0));
    }
    response
        .rate_limit_reset
        .as_deref()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .map(|reset| (reset - now).max(1))
}

/// The complete fetch: one profile request, then the owned-repository pages,
/// serially, until the provider reports no next page. Every repository page
/// count is checked arithmetic; a traversal that cannot complete within the
/// page budget fails as [`GithubError::Incomplete`] and persists nothing.
fn fetch_stats_with_transport(
    transport: &impl HttpTransport,
    username: &str,
    fetched_at: i64,
) -> Result<GithubSnapshot, GithubError> {
    let profile = transport.get(profile_url(username)?)?;
    let user: RawUser = match profile.status {
        200 => serde_json::from_str(&profile.body).map_err(|_| GithubError::InvalidResponse)?,
        404 => return Err(GithubError::NotFound),
        _ => return Err(refused_status_error(&profile, fetched_at)),
    };

    let mut stars: u64 = 0;
    let mut pages = 0usize;
    let mut next = Some(repositories_url(username)?);
    while let Some(url) = next {
        if pages >= MAX_REPOSITORY_PAGES {
            return Err(GithubError::Incomplete);
        }
        let response = transport.get(url)?;
        let repositories: Vec<RawRepository> = match response.status {
            200 => serde_json::from_str(&response.body).map_err(|_| GithubError::InvalidResponse)?,
            _ => return Err(refused_status_error(&response, fetched_at)),
        };
        for repository in &repositories {
            // `type=owner` already scopes to owned repositories and the
            // public API only serves public ones; the flags are checked
            // anyway so the aggregation rule holds against any future
            // response shape: owned + public + non-fork, archived included.
            if repository.fork || repository.private {
                continue;
            }
            stars = stars
                .checked_add(repository.stargazers_count)
                .ok_or(GithubError::InvalidResponse)?;
        }
        pages += 1;
        next = next_page_url(&response)?;
    }

    Ok(GithubSnapshot {
        username: user.login,
        followers: user.followers,
        stars,
        fetched_at,
    })
}

#[tauri::command]
pub fn github_state(
    app_state: tauri::State<'_, AppState>,
    runtime: tauri::State<'_, GithubRuntime>,
) -> Result<GithubViewState, GithubError> {
    github_view(&app_state, &runtime, unix_timestamp())
}

#[tauri::command]
pub async fn refresh_github(
    app: tauri::AppHandle,
    manual: bool,
) -> Result<GithubViewState, GithubError> {
    let runtime = app.state::<GithubRuntime>();
    let settings = app
        .state::<AppState>()
        .snapshot()
        .map_err(|_| GithubError::ProviderError)?;
    // Both configuration gates run before the refresh gate: an unconfigured
    // or invalid username is refused without touching the single-flight flag,
    // the cooldown, or the network.
    let username = normalize_username(&settings.github_username)?;
    runtime.begin_refresh(manual)?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        let transport = ReqwestTransport::new()?;
        fetch_stats_with_transport(&transport, &username, unix_timestamp())
    })
    .await
    .map_err(|_| GithubError::ProviderError)
    .and_then(|value| value);

    runtime.finish_refresh();
    match result {
        Ok(snapshot) => {
            let payload_json =
                serde_json::to_string(&snapshot).map_err(|_| GithubError::InvalidResponse)?;
            app.state::<AppState>()
                .database
                .upsert_github_cache(&GithubCacheRow {
                    username: cache_key(&snapshot.username),
                    payload_json,
                    fetched_at: snapshot.fetched_at,
                })
                .map_err(|_| GithubError::ProviderError)?;
            runtime.record("success");
            github_view(&app.state::<AppState>(), &runtime, unix_timestamp())
        }
        Err(error) => {
            if let GithubError::RateLimited(seconds) = &error {
                runtime.note_rate_limit(*seconds);
            }
            // The username is deliberately omitted: normal logs expose only
            // the provider outcome class, never the configured account.
            eprintln!("[github] result={}", error.diagnostic_label());
            runtime.record(error.diagnostic_label());
            Err(error)
        }
    }
}

/// Opens the configured account's public GitHub profile in the system
/// browser. The username is re-validated here and the URL is built from the
/// fixed profile origin only — the frontend supplies the username string,
/// never a URL.
#[tauri::command]
pub fn open_github_profile(username: String) -> Result<(), String> {
    let username = username.trim();
    validate_username(username)?;
    let mut url = Url::parse(PROFILE_ORIGIN).map_err(|error| error.to_string())?;
    url.path_segments_mut()
        .map_err(|_| "url path error".to_string())?
        .pop_if_empty()
        .push(username);
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer.exe")
            .arg(url.as_str())
            .spawn()
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn github_view(
    state: &AppState,
    runtime: &GithubRuntime,
    now: i64,
) -> Result<GithubViewState, GithubError> {
    let settings = state.snapshot().map_err(|_| GithubError::ProviderError)?;
    let username = settings.github_username.trim().to_string();
    if username.is_empty() {
        return Ok(GithubViewState {
            configured: false,
            username: String::new(),
            snapshot: None,
            cache_status: GithubCacheStatus::Missing,
            cache_age_seconds: None,
            refreshing: runtime.refreshing.load(Ordering::Acquire),
            rate_limited: false,
            retry_after_seconds: None,
            last_refresh: runtime.last_refresh(),
        });
    }
    let key = cache_key(&username);
    let row = state
        .database
        .github_cache(&key)
        .map_err(|_| GithubError::ProviderError)?;
    let snapshot = row.as_ref().and_then(|row| {
        serde_json::from_str::<GithubSnapshot>(&row.payload_json)
            .ok()
            .filter(|snapshot| {
                snapshot.username.to_lowercase() == key && snapshot.fetched_at == row.fetched_at
            })
    });
    let age = snapshot
        .as_ref()
        .map(|snapshot| now.saturating_sub(snapshot.fetched_at).max(0));
    let retry_after_seconds = runtime.rate_limit_remaining();
    Ok(GithubViewState {
        configured: true,
        username,
        cache_status: age.map(cache_status).unwrap_or(GithubCacheStatus::Missing),
        cache_age_seconds: age,
        snapshot,
        refreshing: runtime.refreshing.load(Ordering::Acquire),
        rate_limited: retry_after_seconds.is_some(),
        retry_after_seconds,
        last_refresh: runtime.last_refresh(),
    })
}

fn cache_status(age_seconds: i64) -> GithubCacheStatus {
    if age_seconds < FRESH_AFTER_SECONDS {
        GithubCacheStatus::Fresh
    } else {
        GithubCacheStatus::Stale
    }
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    const PROFILE_FIXTURE: &str = r#"{ "login": "octocat", "followers": 12 }"#;

    /// One repository page fixture. `fork`/`private`/`archived` flags and star
    /// counts are the whole aggregation input.
    fn repo_json(fork: bool, private: bool, archived: bool, stars: u64) -> String {
        format!(
            r#"{{ "name": "repo", "fork": {fork}, "private": {private}, "archived": {archived}, "stargazers_count": {stars} }}"#
        )
    }

    fn repos_page(repos: &[String], link: Option<&str>) -> HttpResponse {
        HttpResponse {
            status: 200,
            retry_after: None,
            rate_limit_remaining: None,
            rate_limit_reset: None,
            link: link.map(str::to_string),
            body: format!("[{}]", repos.join(",")),
        }
    }

    fn profile_response(status: u16, body: &str) -> HttpResponse {
        HttpResponse {
            status,
            retry_after: None,
            rate_limit_remaining: None,
            rate_limit_reset: None,
            link: None,
            body: body.to_string(),
        }
    }

    fn status_response(status: u16) -> HttpResponse {
        profile_response(status, "{}")
    }

    /// A transport that answers from a fixed queue, in request order, and
    /// records every URL it was asked for — the serial-pagination witness.
    struct QueueTransport {
        responses: Mutex<Vec<HttpResponse>>,
        calls: AtomicUsize,
        urls: Mutex<Vec<String>>,
        error: Option<GithubError>,
    }

    impl QueueTransport {
        fn new(responses: Vec<HttpResponse>) -> Self {
            Self {
                responses: Mutex::new(responses),
                calls: AtomicUsize::new(0),
                urls: Mutex::new(Vec::new()),
                error: None,
            }
        }

        fn error(error: GithubError) -> Self {
            Self {
                responses: Mutex::new(Vec::new()),
                calls: AtomicUsize::new(0),
                urls: Mutex::new(Vec::new()),
                error: Some(error),
            }
        }

        fn urls(&self) -> Vec<String> {
            self.urls.lock().expect("urls").clone()
        }

        fn response(status: u16, body: &'static str) -> Self {
            Self::new(vec![profile_response(status, body)])
        }
    }

    impl HttpTransport for QueueTransport {
        fn get(&self, url: Url) -> Result<HttpResponse, GithubError> {
            self.calls.fetch_add(1, AtomicOrdering::Relaxed);
            self.urls.lock().expect("urls").push(url.to_string());
            if let Some(error) = &self.error {
                return Err(error.clone());
            }
            self.responses
                .lock()
                .expect("responses")
                .pop()
                .ok_or_else(|| GithubError::ProviderError)
        }
    }

    fn configured_state(username: &str) -> AppState {
        let state = AppState::load(Database::in_memory().expect("database")).expect("state");
        state
            .update(|settings| {
                settings.github_username = username.into();
            })
            .expect("configure");
        state
    }

    // --- Username validation --------------------------------------------------

    #[test]
    fn valid_usernames_pass_the_shape_check() {
        for value in ["a", "octocat", "Octo-Cat", "0-0", &"x".repeat(39)] {
            assert!(validate_username(value).is_ok(), "{value} must pass");
            assert_eq!(normalize_username(value), Ok(value.trim().to_string()));
        }
    }

    #[test]
    fn malformed_usernames_are_refused_before_any_url_exists() {
        for value in [
            "-leading",
            "trailing-",
            "has space",
            "under_score",
            "dot.name",
            "ünicode",
            &"x".repeat(40),
        ] {
            assert!(validate_username(value).is_err(), "{value} must fail");
            assert_eq!(normalize_username(value), Err(GithubError::InvalidUsername));
        }
    }

    #[test]
    fn the_empty_username_is_unconfigured_not_invalid() {
        assert!(validate_username("").is_ok(), "settings may store empty");
        assert!(validate_username("   ").is_ok());
        assert_eq!(normalize_username(""), Err(GithubError::NotConfigured));
        assert_eq!(normalize_username("   "), Err(GithubError::NotConfigured));
        // Whitespace around a valid name normalizes away.
        assert_eq!(normalize_username("  octo-cat "), Ok("octo-cat".into()));
    }

    // --- Request construction --------------------------------------------------

    #[test]
    fn request_urls_are_built_from_the_fixed_api_origin() {
        let profile = profile_url("octo-cat").expect("profile url");
        assert_eq!(profile.as_str(), "https://api.github.com/users/octo-cat");

        let repos = repositories_url("octo-cat").expect("repos url");
        assert_eq!(
            repos.as_str(),
            "https://api.github.com/users/octo-cat/repos?type=owner&per_page=100"
        );
    }

    // --- Aggregation ------------------------------------------------------------

    #[test]
    fn followers_are_read_from_the_profile_response() {
        // The queue is consumed from the end via pop, so the profile fixture
        // is listed last and answers the first request.
        let transport = QueueTransport::new(vec![
            repos_page(&[], None),
            profile_response(200, PROFILE_FIXTURE),
        ]);
        let snapshot = fetch_stats_with_transport(&transport, "octocat", 100).expect("snapshot");
        assert_eq!(snapshot.followers, 12);
        assert_eq!(snapshot.username, "octocat");
        assert_eq!(snapshot.fetched_at, 100);
    }

    #[test]
    fn stars_aggregate_owned_public_non_fork_repositories_archived_included() {
        let transport = QueueTransport::new(vec![
            repos_page(
                &[
                    repo_json(false, false, false, 3),
                    // A fork never counts, however many stars it collected.
                    repo_json(true, false, false, 1000),
                    // Archived repositories are the user's own work and count.
                    repo_json(false, false, true, 7),
                    // A defensively-skipped private row (the public endpoint
                    // does not serve these; the rule must survive shapes).
                    repo_json(false, true, false, 500),
                ],
                None,
            ),
            profile_response(200, PROFILE_FIXTURE),
        ]);
        let snapshot =
            fetch_stats_with_transport(&transport, "octocat", 100).expect("snapshot");
        assert_eq!(snapshot.stars, 10, "3 + 7: forks and private rows excluded");
    }

    #[test]
    fn pagination_follows_next_links_serially_and_stops_without_one() {
        let page_two_url = "https://api.github.com/users/octocat/repos?type=owner&per_page=100&page=2";
        let transport = QueueTransport::new(vec![
            // Request order: profile, page 1, page 2 (pop takes from the end).
            repos_page(&[repo_json(false, false, false, 1)], None),
            repos_page(
                &[repo_json(false, false, false, 2)],
                Some(&format!(r#"<{page_two_url}>; rel="next", <https://api.github.com/users/octocat/repos?type=owner&per_page=100&page=9>; rel="last""#)),
            ),
            profile_response(200, PROFILE_FIXTURE),
        ]);
        let snapshot =
            fetch_stats_with_transport(&transport, "octocat", 100).expect("snapshot");
        assert_eq!(snapshot.stars, 3, "page 1 + page 2");
        let urls = transport.urls();
        assert_eq!(urls.len(), 3);
        assert!(urls[0].ends_with("/users/octocat"), "profile first: {urls:?}");
        assert!(!urls[1].contains("page=2"), "page 1 before page 2: {urls:?}");
        assert!(urls[2].contains("page=2"), "the Link target is followed: {urls:?}");
    }

    #[test]
    fn pagination_beyond_the_budget_fails_incomplete() {
        let next = r#"<https://api.github.com/users/octocat/repos?type=owner&per_page=100&page=2>; rel="next""#;
        // MAX_REPOSITORY_PAGES pages that all claim a next page, after the
        // profile request: the traversal must stop at the budget and refuse.
        let mut responses = vec![profile_response(200, PROFILE_FIXTURE)];
        for _ in 0..MAX_REPOSITORY_PAGES {
            responses.insert(0, repos_page(&[repo_json(false, false, false, 1)], Some(next)));
        }
        let transport = QueueTransport::new(responses);
        assert_eq!(
            fetch_stats_with_transport(&transport, "octocat", 100),
            Err(GithubError::Incomplete)
        );
        assert_eq!(
            transport.calls.load(AtomicOrdering::Relaxed),
            MAX_REPOSITORY_PAGES + 1,
            "one profile request plus exactly the bounded page count"
        );
    }

    #[test]
    fn a_pagination_link_off_the_api_origin_is_refused() {
        for hostile in [
            r#"<https://evil.example/users/octocat/repos?page=2>; rel="next""#,
            r#"<http://api.github.com/users/octocat/repos?page=2>; rel="next""#,
            r#"<https://api.github.com.evil.example/repos?page=2>; rel="next""#,
            r#"<not a url>; rel="next""#,
        ] {
            let transport = QueueTransport::new(vec![
                repos_page(&[], Some(hostile)),
                profile_response(200, PROFILE_FIXTURE),
            ]);
            assert_eq!(
                fetch_stats_with_transport(&transport, "octocat", 100),
                Err(GithubError::InvalidResponse),
                "{hostile} must be refused"
            );
        }
    }

    #[test]
    fn star_arithmetic_overflow_is_an_error_not_a_wrap() {
        let huge = u64::MAX;
        let transport = QueueTransport::new(vec![
            repos_page(
                &[repo_json(false, false, false, huge), repo_json(false, false, false, 5)],
                None,
            ),
            profile_response(200, PROFILE_FIXTURE),
        ]);
        assert_eq!(
            fetch_stats_with_transport(&transport, "octocat", 100),
            Err(GithubError::InvalidResponse)
        );
    }

    // --- HTTP status and error mapping -------------------------------------------

    #[test]
    fn a_missing_account_maps_to_not_found() {
        let transport = QueueTransport::response(404, r#"{"message":"Not Found"}"#);
        assert_eq!(
            fetch_stats_with_transport(&transport, "nobody", 100),
            Err(GithubError::NotFound)
        );
    }

    #[test]
    fn rate_limit_shapes_are_recognized_and_other_statuses_are_not() {
        // 429 → RateLimited.
        let mut rate_limited = status_response(429);
        rate_limited.retry_after = Some("120".into());
        let transport = QueueTransport::new(vec![rate_limited]);
        assert_eq!(
            fetch_stats_with_transport(&transport, "octocat", 100),
            Err(GithubError::RateLimited(Some(120)))
        );

        // 403 with X-RateLimit-Remaining: 0 → RateLimited, reset honored.
        let mut exhausted = status_response(403);
        exhausted.rate_limit_remaining = Some("0".into());
        exhausted.rate_limit_reset = Some("200".into());
        let transport = QueueTransport::new(vec![exhausted]);
        assert_eq!(
            fetch_stats_with_transport(&transport, "octocat", 100),
            Err(GithubError::RateLimited(Some(100))),
            "reset is an absolute epoch; 200 - now(100) = 100s"
        );

        // 403 WITHOUT the exhausted-budget shape (a forbidden resource) is a
        // generic provider error, not a rate-limit lockout.
        let transport = QueueTransport::new(vec![status_response(403)]);
        assert_eq!(
            fetch_stats_with_transport(&transport, "octocat", 100),
            Err(GithubError::ProviderError)
        );

        // 500 → generic provider error.
        let transport = QueueTransport::new(vec![status_response(500)]);
        assert_eq!(
            fetch_stats_with_transport(&transport, "octocat", 100),
            Err(GithubError::ProviderError)
        );
    }

    #[test]
    fn missing_retry_hints_fall_back_to_none() {
        let transport = QueueTransport::new(vec![status_response(429)]);
        assert_eq!(
            fetch_stats_with_transport(&transport, "octocat", 100),
            Err(GithubError::RateLimited(None))
        );
    }

    #[test]
    fn network_timeout_and_malformed_payload_map_to_the_pinned_vocabulary() {
        assert_eq!(
            fetch_stats_with_transport(
                &QueueTransport::error(GithubError::Timeout),
                "octocat",
                100
            ),
            Err(GithubError::Timeout)
        );
        assert_eq!(
            fetch_stats_with_transport(
                &QueueTransport::error(GithubError::Network),
                "octocat",
                100
            ),
            Err(GithubError::Network)
        );
        let transport = QueueTransport::new(vec![profile_response(200, "not json")]);
        assert_eq!(
            fetch_stats_with_transport(&transport, "octocat", 100),
            Err(GithubError::InvalidResponse)
        );
    }

    // --- View, cache, and runtime -------------------------------------------------

    #[test]
    fn unconfigured_view_carries_no_snapshot_and_the_setup_state() {
        let state = configured_state("");
        let runtime = GithubRuntime::default();
        let view = github_view(&state, &runtime, 100).expect("view");
        assert!(!view.configured);
        assert!(view.username.is_empty());
        assert_eq!(view.cache_status, GithubCacheStatus::Missing);
        assert!(view.snapshot.is_none());
        assert!(!view.rate_limited);
    }

    #[test]
    fn cache_reports_missing_fresh_and_stale() {
        let state = configured_state("octocat");
        let runtime = GithubRuntime::default();
        assert_eq!(
            github_view(&state, &runtime, 100).expect("missing").cache_status,
            GithubCacheStatus::Missing
        );
        for (age, expected) in [
            (60, GithubCacheStatus::Fresh),
            (FRESH_AFTER_SECONDS, GithubCacheStatus::Stale),
            (31 * 60, GithubCacheStatus::Stale),
        ] {
            let snapshot = GithubSnapshot {
                username: "octocat".into(),
                followers: 1,
                stars: 2,
                fetched_at: 100_000 - age,
            };
            state
                .database
                .upsert_github_cache(&GithubCacheRow {
                    username: cache_key("octocat"),
                    payload_json: serde_json::to_string(&snapshot).expect("json"),
                    fetched_at: snapshot.fetched_at,
                })
                .expect("cache");
            let view = github_view(&state, &runtime, 100_000).expect("view");
            assert_eq!(view.cache_status, expected, "age {age}");
            assert_eq!(view.cache_age_seconds, Some(age));
        }
    }

    #[test]
    fn a_corrupt_or_mismatched_cache_row_is_ignored_as_missing() {
        let state = configured_state("octocat");
        let runtime = GithubRuntime::default();
        state
            .database
            .upsert_github_cache(&GithubCacheRow {
                username: cache_key("octocat"),
                payload_json: "corrupt".into(),
                fetched_at: 90,
            })
            .expect("corrupt cache");
        assert!(github_view(&state, &runtime, 100)
            .expect("view")
            .snapshot
            .is_none());

        // A row whose payload names a different account is not this account's
        // snapshot, even though it sits under a matching key's neighbor.
        let snapshot = GithubSnapshot {
            username: "someone-else".into(),
            followers: 1,
            stars: 2,
            fetched_at: 90,
        };
        state
            .database
            .upsert_github_cache(&GithubCacheRow {
                username: cache_key("someone-else"),
                payload_json: serde_json::to_string(&snapshot).expect("json"),
                fetched_at: 90,
            })
            .expect("other row");
        let view = github_view(&state, &runtime, 100).expect("view");
        assert!(view.snapshot.is_none());
    }

    #[test]
    fn a_username_change_starts_from_an_empty_cache() {
        let state = configured_state("octocat");
        let runtime = GithubRuntime::default();
        let snapshot = GithubSnapshot {
            username: "octocat".into(),
            followers: 1,
            stars: 2,
            fetched_at: 90,
        };
        state
            .database
            .upsert_github_cache(&GithubCacheRow {
                username: cache_key("octocat"),
                payload_json: serde_json::to_string(&snapshot).expect("json"),
                fetched_at: 90,
            })
            .expect("cache");
        assert!(github_view(&state, &runtime, 100)
            .expect("old account")
            .snapshot
            .is_some());

        state
            .update(|settings| settings.github_username = "OctoCat".into())
            .expect("change username");
        // Same account, different spelling: the cache key is the lowercase
        // username, so the case-variant spelling reads the same row…
        let view = github_view(&state, &runtime, 100).expect("case-variant");
        assert_eq!(view.username, "OctoCat");
        assert!(view.snapshot.is_some(), "case-insensitive cache key");

        // …while a genuinely different account has nothing.
        state
            .update(|settings| settings.github_username = "someone-else".into())
            .expect("switch account");
        let view = github_view(&state, &runtime, 100).expect("new account");
        assert_eq!(view.cache_status, GithubCacheStatus::Missing);
        assert!(view.snapshot.is_none());
    }

    #[test]
    fn refresh_runtime_deduplicates_and_cools_down_manual_refreshes() {
        let runtime = GithubRuntime::default();
        runtime.begin_refresh(false).expect("first");
        assert_eq!(
            runtime.begin_refresh(false),
            Err(GithubError::InProgress),
            "a concurrent identical fetch is deduplicated"
        );
        runtime.finish_refresh();
        runtime.begin_refresh(true).expect("manual");
        runtime.finish_refresh();
        assert_eq!(
            runtime.begin_refresh(true),
            Err(GithubError::Cooldown),
            "a manual refresh inside the cooldown is refused"
        );
        // Background (non-manual) refreshes are not cooled down.
        runtime.begin_refresh(false).expect("background refresh");
        runtime.finish_refresh();
    }

    #[test]
    fn a_rate_limit_backoff_refuses_refreshes_without_network_activity() {
        let runtime = GithubRuntime::default();
        runtime.note_rate_limit(Some(120));
        let view_remaining = runtime.rate_limit_remaining().expect("backoff in force");
        assert!(view_remaining > 100 && view_remaining <= 120);
        assert_eq!(
            runtime.begin_refresh(false),
            Err(GithubError::RateLimited(Some(view_remaining))),
        );
        assert_eq!(
            runtime.begin_refresh(true),
            Err(GithubError::RateLimited(Some(view_remaining))),
            "manual refreshes are refused during the backoff too"
        );

        // A hostile hint cannot outlive the cap.
        runtime.note_rate_limit(Some(10 * 24 * 60 * 60));
        let remaining = runtime.rate_limit_remaining().expect("backoff");
        assert!(
            remaining <= RATE_LIMIT_BACKOFF_CAP.as_secs() as i64,
            "the backoff must be capped, got {remaining}s"
        );
    }

    #[test]
    fn the_view_exposes_the_rate_limit_state() {
        let state = configured_state("octocat");
        let runtime = GithubRuntime::default();
        runtime.note_rate_limit(Some(120));
        let view = github_view(&state, &runtime, unix_timestamp()).expect("view");
        assert!(view.rate_limited);
        assert!(view.retry_after_seconds.is_some());
    }
}
