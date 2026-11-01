use crate::{
    database::{GithubCacheRow, GithubCommitsCacheRow, GithubSearchCacheRow},
    settings::AppState,
};
use chrono::{DateTime, Local, LocalResult, Offset, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
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

/// The search-index snapshot (v1.3.0-G2): two account-wide totals read from
/// GitHub's public search index — currently open issues authored by the
/// account anywhere GitHub's index can see, and all pull requests it has
/// authored (open, merged, or closed unmerged; each counted once). The data
/// source is the searchable PUBLIC index by definition: these numbers never
/// represent private, deleted, or otherwise inaccessible activity. One
/// snapshot always covers both metrics atomically — a refresh that cannot
/// complete both search queries persists nothing, mirroring the G1
/// complete-traversal-or-nothing rule.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchSnapshot {
    /// The validated username the queries were built from (the configured
    /// spelling, trimmed). The search API names no canonical login for a
    /// `total_count`-only read, so the integrity filter checks this field
    /// against the row's cache key instead.
    pub username: String,
    pub open_issues: u64,
    pub pull_requests: u64,
    pub fetched_at: i64,
}

/// The today's-commits snapshot (v1.3.0-G3): the count of commits GitHub's
/// public commit-search index attributes to the configured account as author,
/// on default branches of public repositories, with an author timestamp
/// between this machine's local midnight and the fetch instant. The data
/// source is the searchable PUBLIC index by definition: the number never
/// represents private, non-default-branch, or not-yet-indexed work, and it is
/// a count of index occurrences, not an exhaustive unique-SHA total. The
/// snapshot is valid for one local calendar day in one timezone: both the day
/// string and the exact midnight instant below are part of the identity, and
/// the view refuses a snapshot whose either has moved.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitsSnapshot {
    /// The validated username the query was built from (the configured
    /// spelling, trimmed). The search API names no canonical login for a
    /// `total_count`-only read, so the integrity filter checks this field
    /// against the row's cache key instead.
    pub username: String,
    pub commits: u64,
    /// The local calendar day the count covers, `YYYY-MM-DD` in the runtime's
    /// local timezone at fetch time.
    pub local_day: String,
    /// The exact instant (unix seconds) the count's author-date range starts:
    /// this machine's local midnight. Recomputed by every view read; a
    /// snapshot computed for a different midnight — a date roll or a machine
    /// timezone change — is refused even when its day string still matches.
    pub local_day_start: i64,
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
    /// The search-index plane (G2), independent of the profile plane above in
    /// every respect: its own cache row and validity, its own refresh flag,
    /// and its own command. Only the provider rate-limit backoff is shared —
    /// whichever plane trips it stops both, so a hot plane cannot hammer a
    /// provider that just asked for a pause.
    pub search: Option<SearchSnapshot>,
    pub search_cache_status: GithubCacheStatus,
    pub search_cache_age_seconds: Option<i64>,
    pub search_refreshing: bool,
    /// The search plane's last-refresh outcome, the `last_refresh` string's
    /// counterpart (QA evidence, never rendered).
    pub search_last_refresh: String,
    /// The commits plane (G3), independent of both planes above in every
    /// respect — its own cache row and validity, its own refresh flag, and its
    /// own command. Its validity adds the local-day axis: a snapshot computed
    /// for a previous calendar day (or a moved machine-timezone midnight)
    /// renders as missing, never as today's count. The provider rate-limit
    /// backoff and the search-request budget guard are shared with the search
    /// plane.
    pub commits: Option<CommitsSnapshot>,
    pub commits_cache_status: GithubCacheStatus,
    pub commits_cache_age_seconds: Option<i64>,
    pub commits_refreshing: bool,
    pub commits_last_refresh: String,
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
    /// A traversal could not complete within its budget: the repository
    /// pagination ran out of pages, or a search response declared
    /// `incomplete_results`. Never persisted, on either plane: the previous
    /// complete snapshot survives instead.
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

/// The two Search-API planes (G2, G3) draw on the provider's per-minute
/// search budget — 10 requests per minute unauthenticated, a separate
/// resource from the core budget G1 uses. Independent single-flight flags
/// keep each plane from colliding with itself; they say nothing about the
/// shared budget. This coordinator adds the two smallest safe facts:
///
/// 1. Search requests never fly concurrently — a per-request lock serializes
///    G2's two requests and G3's one against each other (each plane's own
///    fetches are already single-flighted, and one request at a time inside
///    a fetch keeps G2's two-request sequence intact).
/// 2. Every attempted dispatch is counted in a sliding 60-second window that
///    refuses further search requests once the documented budget is reached —
///    before the provider has to. Attempts are counted, not successes: a
///    network failure spends nothing at the provider, but counting it is the
///    conservative reading of a budget this small. A refusal carries the
///    window's real remaining seconds as the retry hint, so it feeds the same
///    shared rate-limit backoff the provider's own refusals do.
struct SearchCoordinator {
    /// Held for the duration of each single search request.
    serial: Mutex<()>,
    /// Dispatch instants of attempted search requests inside the window.
    window: Mutex<VecDeque<Instant>>,
}

/// The documented unauthenticated search budget the window never exceeds.
const SEARCH_REQUESTS_PER_WINDOW: usize = 10;
/// The window's length; a dispatch leaves the guard one full window later.
const SEARCH_WINDOW: Duration = Duration::from_secs(60);

impl Default for SearchCoordinator {
    fn default() -> Self {
        Self {
            serial: Mutex::new(()),
            window: Mutex::new(VecDeque::new()),
        }
    }
}

impl SearchCoordinator {
    /// One guarded search request: serialized against the other plane's
    /// current request, admitted by the sliding budget window, then fetched
    /// through the caller's transport unchanged.
    fn guarded_get(
        &self,
        transport: &impl HttpTransport,
        url: Url,
    ) -> Result<HttpResponse, GithubError> {
        let _serial = self.serial.lock().map_err(|_| GithubError::ProviderError)?;
        self.request_permit(Instant::now())?;
        transport.get(url)
    }

    /// The sliding-window admission decision, split out so tests can drive
    /// it with chosen clock readings. Prunes expired dispatches, refuses a
    /// full window with the time its oldest entry still holds, otherwise
    /// records this dispatch and admits it.
    fn request_permit(&self, now: Instant) -> Result<(), GithubError> {
        let mut window = self.window.lock().map_err(|_| GithubError::ProviderError)?;
        while window
            .front()
            .is_some_and(|dispatched| now.duration_since(*dispatched) >= SEARCH_WINDOW)
        {
            window.pop_front();
        }
        if window.len() >= SEARCH_REQUESTS_PER_WINDOW {
            let oldest = window.front().expect("a full window is nonempty");
            let wait = SEARCH_WINDOW
                .checked_sub(now.duration_since(*oldest))
                .unwrap_or_default();
            return Err(GithubError::RateLimited(Some(
                wait.as_secs().max(1) as i64
            )));
        }
        window.push_back(now);
        Ok(())
    }
}

#[derive(Default)]
pub struct GithubRuntime {
    refreshing: AtomicBool,
    /// The search plane's single-flight flag, deliberately separate from
    /// `refreshing`: the two commands are independent jobs against different
    /// provider rate budgets, and one plane's in-flight fetch must not make
    /// the other refuse as `InProgress`.
    search_refreshing: AtomicBool,
    /// The commits plane's single-flight flag (G3), separate for the same
    /// reason: three independent jobs, three independent flags.
    commits_refreshing: AtomicBool,
    last_manual_refresh: Mutex<Option<Instant>>,
    last_manual_search_refresh: Mutex<Option<Instant>>,
    last_manual_commits_refresh: Mutex<Option<Instant>>,
    rate_limited_until: Mutex<Option<Instant>>,
    last_refresh: Mutex<String>,
    last_search_refresh: Mutex<String>,
    last_commits_refresh: Mutex<String>,
    /// The shared search-request coordinator: both Search-API planes'
    /// requests serialize through it and are counted against one window.
    search_coordinator: SearchCoordinator,
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

    fn last_search_refresh(&self) -> String {
        self.last_search_refresh
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

    fn record_search(&self, value: &str) {
        if let Ok(mut status) = self.last_search_refresh.lock() {
            *status = value.into();
        }
    }

    /// The search plane's refresh gate: the same shape as [`Self::begin_refresh`]
    /// (rate-limit gate first, then the manual cooldown, then single-flight)
    /// with its own flag and cooldown clock, and the shared rate-limit backoff.
    /// The backoff is shared on purpose — whichever plane the provider asked
    /// to pause, both must pause.
    fn begin_search_refresh(&self, manual: bool) -> Result<(), GithubError> {
        if let Some(remaining) = self.rate_limit_remaining() {
            return Err(GithubError::RateLimited(Some(remaining)));
        }
        if manual {
            let mut last = self
                .last_manual_search_refresh
                .lock()
                .map_err(|_| GithubError::ProviderError)?;
            if last.is_some_and(|instant| instant.elapsed() < MANUAL_REFRESH_COOLDOWN) {
                return Err(GithubError::Cooldown);
            }
            *last = Some(Instant::now());
        }
        if self.search_refreshing.swap(true, Ordering::AcqRel) {
            return Err(GithubError::InProgress);
        }
        Ok(())
    }

    fn finish_search_refresh(&self) {
        self.search_refreshing.store(false, Ordering::Release);
    }

    fn last_commits_refresh(&self) -> String {
        self.last_commits_refresh
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

    fn record_commits(&self, value: &str) {
        if let Ok(mut status) = self.last_commits_refresh.lock() {
            *status = value.into();
        }
    }

    /// The commits plane's refresh gate: the same shape as the other two
    /// planes (rate-limit gate first, then the manual cooldown, then
    /// single-flight) with its own flag and cooldown clock, and the shared
    /// rate-limit backoff.
    fn begin_commits_refresh(&self, manual: bool) -> Result<(), GithubError> {
        if let Some(remaining) = self.rate_limit_remaining() {
            return Err(GithubError::RateLimited(Some(remaining)));
        }
        if manual {
            let mut last = self
                .last_manual_commits_refresh
                .lock()
                .map_err(|_| GithubError::ProviderError)?;
            if last.is_some_and(|instant| instant.elapsed() < MANUAL_REFRESH_COOLDOWN) {
                return Err(GithubError::Cooldown);
            }
            *last = Some(Instant::now());
        }
        if self.commits_refreshing.swap(true, Ordering::AcqRel) {
            return Err(GithubError::InProgress);
        }
        Ok(())
    }

    fn finish_commits_refresh(&self) {
        self.commits_refreshing.store(false, Ordering::Release);
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

/// Maps a refused profile/page/search status to the pinned error vocabulary:
/// rate-limit shapes are recognized exactly (429; 403 with the remaining
/// budget at zero; 403 carrying a `Retry-After`, the provider's documented
/// secondary-rate-limit signature — search responses use that shape, and
/// pausing on it is the conservative reading of every plane's contract);
/// everything else non-2xx is the generic provider error.
fn refused_status_error(response: &HttpResponse, now: i64) -> GithubError {
    let remaining_zero = response.rate_limit_remaining.as_deref() == Some("0");
    let retry_after_present = response.retry_after.is_some();
    if response.status == 429
        || (response.status == 403 && (remaining_zero || retry_after_present))
    {
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

#[derive(Deserialize)]
struct RawSearchResult {
    total_count: u64,
    incomplete_results: bool,
}

/// The search-index query for one metric (G2). `is:issue is:open` selects
/// only currently open issues authored by the account — GitHub's issue search
/// excludes pull requests from `is:issue`, and nothing else narrows the
/// repository scope, so the count is account-wide across every public
/// repository the index can see. `is:pr` with no state filter selects every
/// pull request the account authored — open, merged, or closed unmerged —
/// each once. `per_page=1` because only `total_count` is ever read; result
/// items are never fetched, listed, or paged.
fn search_query(username: &str, pull_requests: bool) -> String {
    if pull_requests {
        format!("is:pr author:{username}")
    } else {
        format!("is:issue is:open author:{username}")
    }
}

fn search_issues_url(username: &str) -> Result<Url, GithubError> {
    api_url(
        &["search", "issues"],
        &[
            ("q", search_query(username, false).as_str()),
            ("per_page", "1"),
        ],
    )
}

fn search_pull_requests_url(username: &str) -> Result<Url, GithubError> {
    api_url(
        &["search", "issues"],
        &[
            ("q", search_query(username, true).as_str()),
            ("per_page", "1"),
        ],
    )
}

/// One search-index read: the fixed query for `url`'s metric, answered with
/// only the two aggregate fields. `per_page=1` bounds the body; `items` are
/// never requested in volume and never read. The request flies through the
/// shared search coordinator (serialized, budget-guarded) like every other
/// Search-API request.
fn fetch_search_query_with_transport(
    coordinator: &SearchCoordinator,
    transport: &impl HttpTransport,
    url: Url,
    now: i64,
) -> Result<u64, GithubError> {
    let response = coordinator.guarded_get(transport, url)?;
    match response.status {
        200 => {
            let result: RawSearchResult = serde_json::from_str(&response.body)
                .map_err(|_| GithubError::InvalidResponse)?;
            if result.incomplete_results {
                // The provider itself says the index shard was incomplete.
                // The count is unusable by contract: fail the refresh and
                // keep the previous complete snapshot rather than presenting
                // a partial index answer as an exact total.
                return Err(GithubError::Incomplete);
            }
            Ok(result.total_count)
        }
        // Every non-200 here is a refused request, mapped by the shared
        // vocabulary. A nonexistent author can answer 422 Validation Failed
        // ("The listed users cannot be searched…") — a generic provider
        // error, so the account-existence fact stays with the profile plane's
        // 404. An existing account with no indexed activity answers 200 with
        // total_count 0, and that zero is genuine data, never missing.
        _ => Err(refused_status_error(&response, now)),
    }
}

/// The complete search fetch (G2): exactly two Search API requests, serial,
/// issues first. Both must succeed completely or nothing is persisted — one
/// snapshot always covers both metrics.
fn fetch_search_totals_with_transport(
    coordinator: &SearchCoordinator,
    transport: &impl HttpTransport,
    username: &str,
    fetched_at: i64,
) -> Result<SearchSnapshot, GithubError> {
    let open_issues = fetch_search_query_with_transport(
        coordinator,
        transport,
        search_issues_url(username)?,
        fetched_at,
    )?;
    let pull_requests = fetch_search_query_with_transport(
        coordinator,
        transport,
        search_pull_requests_url(username)?,
        fetched_at,
    )?;
    Ok(SearchSnapshot {
        username: username.to_string(),
        open_issues,
        pull_requests,
        fetched_at,
    })
}

/// The ISO-8601 shape the Search API's date qualifiers accept, including the
/// explicit UTC offset the local-day boundaries travel with.
const COMMITS_TIME_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%:z";

/// Resolves the instant the local calendar day of `local_now` begins, for
/// any timezone. The runtime's local timezone is the authority for this
/// desktop installation; the resolver is generic so the DST branches are
/// testable without controlling the process timezone.
///
/// - A normal midnight resolves directly.
/// - A fall-back transition landing exactly on midnight makes the wall-clock
///   00:00:00 happen twice: the local day begins at its first occurrence
///   (the earliest instant), which is what "00:00 today" names.
/// - A spring-forward transition skipping midnight makes the wall-clock
///   00:00:00 nonexistent. The earliest instant of that local day is the
///   transition instant itself, and it is found by probing: the instant
///   wall midnight would be at the offset in force now sits one shift gap
///   BEFORE the transition, still inside the previous day, so the probe
///   re-aims at the pre-shift offset it observes and lands on the
///   transition instant, whose own local wall time is midnight plus the gap
///   — inside the requested day. Two probes converge for any gap shorter
///   than a day; the iteration cap is purely defensive.
fn resolve_day_start<Tz: TimeZone>(tz: &Tz, local_now: &DateTime<Tz>) -> DateTime<Tz> {
    let naive_midnight = local_now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("00:00:00 is a valid wall-clock time");
    match tz.from_local_datetime(&naive_midnight) {
        LocalResult::Single(day_start) => day_start,
        LocalResult::Ambiguous(first_occurrence, _) => first_occurrence,
        LocalResult::None => {
            let today = local_now.date_naive();
            let mut probe = naive_midnight - local_now.offset().fix();
            for _ in 0..4 {
                let offset = tz.offset_from_utc_datetime(&probe).fix();
                if (probe + offset).date() == today {
                    return tz.from_utc_datetime(&probe);
                }
                probe = naive_midnight - offset;
            }
            tz.from_utc_datetime(&probe)
        }
    }
}

/// The local-day context an instant belongs to, in the runtime's local
/// timezone: the `YYYY-MM-DD` day key and the unix second that day's
/// midnight starts. The two are a deterministic pair — the same calendar day
/// resolves to the same midnight instant no matter when during the day it is
/// asked — and together they are the snapshot-validity identity the view
/// checks. An unresolvable instant (never reachable for a real clock
/// reading) degrades to a day key no snapshot can match, which renders the
/// plane as missing instead of failing the view.
fn local_day_context(now: i64) -> (String, i64) {
    let local_now = match Local.timestamp_opt(now, 0).single() {
        Some(local_now) => local_now,
        None => return (String::new(), now),
    };
    let day_key = local_now.date_naive().format("%Y-%m-%d").to_string();
    let day_start = resolve_day_start(&Local, &local_now)
        .with_timezone(&Utc)
        .timestamp();
    (day_key, day_start)
}

/// The local wall-clock form of a unix instant, for the query's range
/// endpoints. `None` is unreachable for real clock readings; callers refuse
/// rather than build a query from a degraded value.
fn local_datetime_at(now: i64) -> Option<DateTime<Local>> {
    Local.timestamp_opt(now, 0).single()
}

/// The commit-search query (G3): one account-wide author-date range, from
/// this machine's local midnight to the fetch instant. `author:` carries the
/// attribution (GitHub's own email-to-account linkage, the same mechanism
/// the contribution graph uses), `is:public` pins the visible scope, and the
/// `author-date` range — supported as ISO-8601 timestamps with explicit
/// offsets — bounds the count to commits authored on this local calendar day
/// so far. The upper bound is the fetch instant, so a commit whose author
/// timestamp falls after the fetch time is never counted; a backdated or
/// re-pushed commit follows its recorded author date, matching what the
/// qualifier filters on.
///
/// Endpoint inclusivity is a documented approximation, never claimed exact:
/// GitHub documents `..` for numbers as covering both endpoints and shows
/// time+offset range examples for dates, but does not separately document
/// per-second endpoint behavior for date ranges — a commit authored exactly
/// at a boundary second may fall either way.
fn commits_query(username: &str, day_start: &DateTime<Local>, fetched_at: i64) -> String {
    let fetch_instant = local_datetime_at(fetched_at)
        .unwrap_or_else(|| day_start.with_timezone(&Local))
        .fixed_offset();
    format!(
        "author:{username} is:public author-date:{}..{}",
        resolve_day_start(&Local, day_start).format(COMMITS_TIME_FORMAT),
        fetch_instant.format(COMMITS_TIME_FORMAT),
    )
}

fn commits_search_url(username: &str, fetched_at: i64) -> Result<Url, GithubError> {
    let day_start = local_datetime_at(fetched_at).ok_or(GithubError::ProviderError)?;
    let query = commits_query(username, &day_start, fetched_at);
    api_url(
        &["search", "commits"],
        &[("q", query.as_str()), ("per_page", "1")],
    )
}

/// The complete commits fetch (G3): exactly one Search API request through
/// the shared coordinator, `per_page=1`, only `total_count` and
/// `incomplete_results` read. A genuine zero is data; an incomplete answer
/// is refused and persists nothing, so a stored snapshot is always a
/// complete answer for the day and midnight it names.
fn fetch_commits_with_transport(
    coordinator: &SearchCoordinator,
    transport: &impl HttpTransport,
    username: &str,
    fetched_at: i64,
) -> Result<CommitsSnapshot, GithubError> {
    let (local_day, local_day_start) = local_day_context(fetched_at);
    if local_day.is_empty() {
        return Err(GithubError::ProviderError);
    }
    let response = coordinator.guarded_get(transport, commits_search_url(username, fetched_at)?)?;
    let commits = match response.status {
        200 => {
            let result: RawSearchResult = serde_json::from_str(&response.body)
                .map_err(|_| GithubError::InvalidResponse)?;
            if result.incomplete_results {
                // The provider itself says the index shard was incomplete.
                // The count is unusable by contract: fail the refresh and
                // keep the previous complete snapshot rather than presenting
                // a partial index answer as an exact total.
                return Err(GithubError::Incomplete);
            }
            result.total_count
        }
        // Every non-200 here is a refused request, mapped by the shared
        // vocabulary (429 and the 403 rate-limit shapes included). A
        // nonexistent author cannot prove itself here — that fact stays with
        // the profile plane's 404, exactly as on the search plane.
        _ => return Err(refused_status_error(&response, fetched_at)),
    };
    Ok(CommitsSnapshot {
        username: username.to_string(),
        commits,
        local_day,
        local_day_start,
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

/// The search plane's refresh (v1.3.0-G2): the same command shape and the
/// same gates as `refresh_github`, costing exactly two Search API requests
/// (the G2 budget ceiling), persisting to its own cache table, and returning
/// the combined view. A search failure never touches the profile plane's
/// cache row, and vice versa.
#[tauri::command]
pub async fn refresh_github_search(
    app: tauri::AppHandle,
    manual: bool,
) -> Result<GithubViewState, GithubError> {
    let runtime = app.state::<GithubRuntime>();
    let settings = app
        .state::<AppState>()
        .snapshot()
        .map_err(|_| GithubError::ProviderError)?;
    // Both configuration gates run before the refresh gate, exactly as the
    // profile command does: an unconfigured or invalid username reaches no
    // search request, flag, or cooldown clock.
    let username = normalize_username(&settings.github_username)?;
    runtime.begin_search_refresh(manual)?;
    // The fetch thread reaches the shared search coordinator through its own
    // handle: the command's `app` stays with this task for the persistence
    // and view half.
    let task_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let transport = ReqwestTransport::new()?;
        let coordinator = &task_app.state::<GithubRuntime>().search_coordinator;
        fetch_search_totals_with_transport(coordinator, &transport, &username, unix_timestamp())
    })
    .await
    .map_err(|_| GithubError::ProviderError)
    .and_then(|value| value);

    runtime.finish_search_refresh();
    match result {
        Ok(snapshot) => {
            let payload_json =
                serde_json::to_string(&snapshot).map_err(|_| GithubError::InvalidResponse)?;
            app.state::<AppState>()
                .database
                .upsert_github_search_cache(&GithubSearchCacheRow {
                    username: cache_key(&snapshot.username),
                    payload_json,
                    fetched_at: snapshot.fetched_at,
                })
                .map_err(|_| GithubError::ProviderError)?;
            runtime.record_search("success");
            github_view(&app.state::<AppState>(), &runtime, unix_timestamp())
        }
        Err(error) => {
            if let GithubError::RateLimited(seconds) = &error {
                runtime.note_rate_limit(*seconds);
            }
            // The username is deliberately omitted: normal logs expose only
            // the provider outcome class, never the configured account.
            eprintln!("[github-search] result={}", error.diagnostic_label());
            runtime.record_search(error.diagnostic_label());
            Err(error)
        }
    }
}

/// The commits plane's refresh (v1.3.0-G3): the same command shape and the
/// same gates as the other two planes, costing exactly one Search API request
/// (the G3 budget ceiling) through the shared search coordinator, persisting
/// to its own cache table, and returning the combined view. A commits failure
/// never touches the profile or search planes' cache rows, and vice versa.
#[tauri::command]
pub async fn refresh_github_commits(
    app: tauri::AppHandle,
    manual: bool,
) -> Result<GithubViewState, GithubError> {
    let runtime = app.state::<GithubRuntime>();
    let settings = app
        .state::<AppState>()
        .snapshot()
        .map_err(|_| GithubError::ProviderError)?;
    // Both configuration gates run before the refresh gate, exactly as the
    // other two commands do: an unconfigured or invalid username reaches no
    // commits request, flag, or cooldown clock.
    let username = normalize_username(&settings.github_username)?;
    runtime.begin_commits_refresh(manual)?;
    // The fetch thread reaches the shared search coordinator through its own
    // handle: the command's `app` stays with this task for the persistence
    // and view half.
    let task_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let transport = ReqwestTransport::new()?;
        let coordinator = &task_app.state::<GithubRuntime>().search_coordinator;
        fetch_commits_with_transport(coordinator, &transport, &username, unix_timestamp())
    })
    .await
    .map_err(|_| GithubError::ProviderError)
    .and_then(|value| value);

    runtime.finish_commits_refresh();
    match result {
        Ok(snapshot) => {
            let payload_json =
                serde_json::to_string(&snapshot).map_err(|_| GithubError::InvalidResponse)?;
            app.state::<AppState>()
                .database
                .upsert_github_commits_cache(&GithubCommitsCacheRow {
                    username: cache_key(&snapshot.username),
                    payload_json,
                    fetched_at: snapshot.fetched_at,
                })
                .map_err(|_| GithubError::ProviderError)?;
            runtime.record_commits("success");
            github_view(&app.state::<AppState>(), &runtime, unix_timestamp())
        }
        Err(error) => {
            if let GithubError::RateLimited(seconds) = &error {
                runtime.note_rate_limit(*seconds);
            }
            // The username is deliberately omitted: normal logs expose only
            // the provider outcome class, never the configured account.
            eprintln!("[github-commits] result={}", error.diagnostic_label());
            runtime.record_commits(error.diagnostic_label());
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
            search: None,
            search_cache_status: GithubCacheStatus::Missing,
            search_cache_age_seconds: None,
            search_refreshing: runtime.search_refreshing.load(Ordering::Acquire),
            search_last_refresh: runtime.last_search_refresh(),
            commits: None,
            commits_cache_status: GithubCacheStatus::Missing,
            commits_cache_age_seconds: None,
            commits_refreshing: runtime.commits_refreshing.load(Ordering::Acquire),
            commits_last_refresh: runtime.last_commits_refresh(),
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
    // The search plane reads its own table under the same integrity rule:
    // the payload must name this account and carry the row's own fetch time.
    let search_row = state
        .database
        .github_search_cache(&key)
        .map_err(|_| GithubError::ProviderError)?;
    let search = search_row.as_ref().and_then(|row| {
        serde_json::from_str::<SearchSnapshot>(&row.payload_json)
            .ok()
            .filter(|snapshot| {
                snapshot.username.to_lowercase() == key && snapshot.fetched_at == row.fetched_at
            })
    });
    let search_age = search
        .as_ref()
        .map(|snapshot| now.saturating_sub(snapshot.fetched_at).max(0));
    // The commits plane reads its own table under the same integrity rule as
    // the search plane, plus the local-day axis: the payload must name this
    // account, carry the row's own fetch time, AND name the calendar day and
    // midnight instant the current local timezone computes for `now`. A
    // snapshot from yesterday — or from a moved machine-timezone midnight
    // that still spells the same date — is refused whole and renders as
    // missing: yesterday's number is never presented as today's count.
    let (today_key, today_start) = local_day_context(now);
    let commits_row = state
        .database
        .github_commits_cache(&key)
        .map_err(|_| GithubError::ProviderError)?;
    let commits = commits_row.as_ref().and_then(|row| {
        serde_json::from_str::<CommitsSnapshot>(&row.payload_json)
            .ok()
            .filter(|snapshot| {
                snapshot.username.to_lowercase() == key
                    && snapshot.fetched_at == row.fetched_at
                    && snapshot.local_day == today_key
                    && snapshot.local_day_start == today_start
            })
    });
    let commits_age = commits
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
        search,
        search_cache_status: search_age
            .map(cache_status)
            .unwrap_or(GithubCacheStatus::Missing),
        search_cache_age_seconds: search_age,
        search_refreshing: runtime.search_refreshing.load(Ordering::Acquire),
        search_last_refresh: runtime.last_search_refresh(),
        commits,
        commits_cache_status: commits_age
            .map(cache_status)
            .unwrap_or(GithubCacheStatus::Missing),
        commits_cache_age_seconds: commits_age,
        commits_refreshing: runtime.commits_refreshing.load(Ordering::Acquire),
        commits_last_refresh: runtime.last_commits_refresh(),
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

    // --- G2: search-index requests ---------------------------------------------

    /// A search-answer fixture: only the two aggregate fields exist for this
    /// feature; `items` are never read and their absence from the fixture
    /// proves the parse does not depend on them.
    fn search_body(total_count: u64, incomplete_results: bool) -> String {
        format!(
            r#"{{ "total_count": {total_count}, "incomplete_results": {incomplete_results} }}"#
        )
    }

    fn search_response(total_count: u64, incomplete_results: bool) -> HttpResponse {
        profile_response(200, &search_body(total_count, incomplete_results))
    }

    fn search_snapshot(username: &str, open_issues: u64, pull_requests: u64, fetched_at: i64) -> SearchSnapshot {
        SearchSnapshot {
            username: username.into(),
            open_issues,
            pull_requests,
            fetched_at,
        }
    }

    fn seed_search_cache(state: &AppState, snapshot: &SearchSnapshot) {
        state
            .database
            .upsert_github_search_cache(&GithubSearchCacheRow {
                username: cache_key(&snapshot.username),
                payload_json: serde_json::to_string(snapshot).expect("json"),
                fetched_at: snapshot.fetched_at,
            })
            .expect("seed search cache");
    }

    #[test]
    fn search_urls_encode_the_queries_under_the_fixed_api_origin() {
        let issues = search_issues_url("octo-cat").expect("issues url");
        assert_eq!(
            issues.as_str(),
            "https://api.github.com/search/issues?q=is%3Aissue+is%3Aopen+author%3Aocto-cat&per_page=1",
            "the query is a single URL-encoded parameter; per_page=1 bounds the body"
        );
        let pull_requests = search_pull_requests_url("octo-cat").expect("pull requests url");
        assert_eq!(
            pull_requests.as_str(),
            "https://api.github.com/search/issues?q=is%3Apr+author%3Aocto-cat&per_page=1"
        );
    }

    #[test]
    fn the_two_search_queries_filter_exactly_as_defined() {
        // Open issues: the open-state filter is part of the query; `is:issue`
        // excludes pull requests by GitHub's search grammar.
        assert_eq!(
            search_query("octocat", false),
            "is:issue is:open author:octocat"
        );
        // All pull requests, each once: no state filter, so open, merged, and
        // closed-unmerged states are all included in the total.
        assert_eq!(search_query("octocat", true), "is:pr author:octocat");
        assert!(
            !search_query("octocat", true).contains("is:open"),
            "the pull-request total must not be narrowed to open states"
        );
    }

    #[test]
    fn search_totals_read_total_count_and_run_serially_within_budget() {
        // Request order: issues first, then pull requests (pop takes from the
        // end, so the issues fixture is listed last).
        let transport = QueueTransport::new(vec![
            search_response(41, false),
            search_response(12, false),
        ]);
        let snapshot =
            fetch_search_totals_with_transport(&SearchCoordinator::default(), &transport, "octocat", 100).expect("snapshot");
        assert_eq!(snapshot.open_issues, 12);
        assert_eq!(snapshot.pull_requests, 41, "total_count is read verbatim");
        assert_eq!(snapshot.username, "octocat");
        assert_eq!(snapshot.fetched_at, 100);
        let urls = transport.urls();
        assert_eq!(urls.len(), 2, "exactly two Search API requests per refresh");
        assert!(
            urls[0].contains("is%3Aissue"),
            "issues are queried first: {urls:?}"
        );
        assert!(
            urls[1].contains("is%3Apr"),
            "pull requests are queried second, serially: {urls:?}"
        );
        assert_eq!(
            transport.calls.load(AtomicOrdering::Relaxed),
            2,
            "the request budget is exactly two search requests"
        );
    }

    #[test]
    fn zero_counts_are_genuine_data_not_missing_data() {
        // An existing account with no indexed activity answers 200 with
        // total_count 0 (a nonexistent author is refused instead, with 422):
        // the zeros are real results and are stored as such.
        let transport = QueueTransport::new(vec![
            search_response(0, false),
            search_response(0, false),
        ]);
        let snapshot =
            fetch_search_totals_with_transport(&SearchCoordinator::default(), &transport, "octocat", 100).expect("snapshot");
        assert_eq!(snapshot.open_issues, 0);
        assert_eq!(snapshot.pull_requests, 0);
    }

    #[test]
    fn an_incomplete_search_answer_is_refused_never_counted() {
        let transport = QueueTransport::new(vec![search_response(1000, true)]);
        assert_eq!(
            fetch_search_totals_with_transport(&SearchCoordinator::default(), &transport, "octocat", 100),
            Err(GithubError::Incomplete),
            "incomplete_results=true is never presented as an exact count"
        );
        assert_eq!(
            transport.calls.load(AtomicOrdering::Relaxed),
            1,
            "an incomplete first answer stops before the second request"
        );
    }

    #[test]
    fn search_refusals_map_to_the_pinned_vocabulary() {
        // 429 → RateLimited with the provider's hint.
        let mut too_many = status_response(429);
        too_many.retry_after = Some("30".into());
        let transport = QueueTransport::new(vec![too_many]);
        assert_eq!(
            fetch_search_totals_with_transport(&SearchCoordinator::default(), &transport, "octocat", 100),
            Err(GithubError::RateLimited(Some(30)))
        );

        // 403 with the remaining budget at zero → RateLimited.
        let mut exhausted = status_response(403);
        exhausted.rate_limit_remaining = Some("0".into());
        exhausted.rate_limit_reset = Some("250".into());
        let transport = QueueTransport::new(vec![exhausted]);
        assert_eq!(
            fetch_search_totals_with_transport(&SearchCoordinator::default(), &transport, "octocat", 100),
            Err(GithubError::RateLimited(Some(150))),
            "reset is an absolute epoch; 250 - now(100) = 150s"
        );

        // 403 with only a Retry-After — the secondary rate-limit signature
        // search responses use — is a rate limit, not a generic refusal.
        let mut secondary = status_response(403);
        secondary.retry_after = Some("60".into());
        let transport = QueueTransport::new(vec![secondary]);
        assert_eq!(
            fetch_search_totals_with_transport(&SearchCoordinator::default(), &transport, "octocat", 100),
            Err(GithubError::RateLimited(Some(60)))
        );

        // 403 without any rate-limit shape stays a generic provider error.
        let transport = QueueTransport::new(vec![status_response(403)]);
        assert_eq!(
            fetch_search_totals_with_transport(&SearchCoordinator::default(), &transport, "octocat", 100),
            Err(GithubError::ProviderError)
        );

        // 422, 404, and 500 are generic provider errors. 422 is reachable
        // with a valid query: GitHub refuses to search a nonexistent author
        // (observed live, "Validation Failed"), which stays a provider error
        // — never the profile plane's NotFound.
        for status in [422, 404, 500] {
            let transport = QueueTransport::new(vec![status_response(status)]);
            assert_eq!(
                fetch_search_totals_with_transport(&SearchCoordinator::default(), &transport, "octocat", 100),
                Err(GithubError::ProviderError),
                "status {status} must be a provider error"
            );
        }
    }

    #[test]
    fn a_malformed_search_answer_is_an_invalid_response() {
        let transport = QueueTransport::new(vec![profile_response(200, "not json")]);
        assert_eq!(
            fetch_search_totals_with_transport(&SearchCoordinator::default(), &transport, "octocat", 100),
            Err(GithubError::InvalidResponse)
        );
        // A missing aggregate field is malformed too.
        let transport = QueueTransport::new(vec![profile_response(200, r#"{ "total_count": 5 }"#)]);
        assert_eq!(
            fetch_search_totals_with_transport(&SearchCoordinator::default(), &transport, "octocat", 100),
            Err(GithubError::InvalidResponse)
        );
    }

    // --- G2: runtime gates ------------------------------------------------------

    #[test]
    fn search_refreshes_deduplicate_and_cool_down_independently() {
        let runtime = GithubRuntime::default();
        runtime.begin_search_refresh(false).expect("first");
        assert_eq!(
            runtime.begin_search_refresh(false),
            Err(GithubError::InProgress),
            "a concurrent identical search fetch is deduplicated"
        );
        runtime.finish_search_refresh();
        // The planes' single-flight flags are independent: the search plane's
        // flight never blocks the profile plane.
        runtime.begin_refresh(false).expect("profile plane unaffected");
        runtime.finish_refresh();

        runtime.begin_search_refresh(true).expect("manual");
        runtime.finish_search_refresh();
        assert_eq!(
            runtime.begin_search_refresh(true),
            Err(GithubError::Cooldown),
            "a manual search refresh inside the cooldown is refused"
        );
        // The cooldown clocks are independent too: the search plane's manual
        // refresh does not consume the profile plane's.
        runtime.begin_refresh(true).expect("independent cooldown clocks");
        runtime.finish_refresh();
        // Background search refreshes are not cooled down.
        runtime.begin_search_refresh(false).expect("background");
        runtime.finish_search_refresh();
    }

    #[test]
    fn a_rate_limit_backoff_is_shared_by_both_planes() {
        let runtime = GithubRuntime::default();
        runtime.note_rate_limit(Some(120));
        let remaining = runtime.rate_limit_remaining().expect("backoff in force");
        assert_eq!(
            runtime.begin_search_refresh(false),
            Err(GithubError::RateLimited(Some(remaining))),
            "the search plane refuses during the shared backoff"
        );
        assert_eq!(
            runtime.begin_refresh(false),
            Err(GithubError::RateLimited(Some(remaining))),
            "so does the profile plane"
        );
    }

    // --- G2: search cache and view ----------------------------------------------

    #[test]
    fn search_cache_reports_missing_fresh_and_stale() {
        let state = configured_state("octocat");
        let runtime = GithubRuntime::default();
        assert_eq!(
            github_view(&state, &runtime, 100)
                .expect("missing")
                .search_cache_status,
            GithubCacheStatus::Missing
        );
        for (age, expected) in [
            (60, GithubCacheStatus::Fresh),
            (FRESH_AFTER_SECONDS, GithubCacheStatus::Stale),
            (31 * 60, GithubCacheStatus::Stale),
        ] {
            let snapshot = search_snapshot("octocat", 1, 2, 100_000 - age);
            seed_search_cache(&state, &snapshot);
            let view = github_view(&state, &runtime, 100_000).expect("view");
            assert_eq!(view.search_cache_status, expected, "age {age}");
            assert_eq!(view.search_cache_age_seconds, Some(age));
            assert_eq!(view.search.as_ref(), Some(&snapshot));
        }
    }

    #[test]
    fn the_two_cache_planes_are_independent() {
        let state = configured_state("octocat");
        let runtime = GithubRuntime::default();

        // G1 success + G2 failure/absence: the profile snapshot renders while
        // the search plane reports missing.
        let profile = GithubSnapshot {
            username: "octocat".into(),
            followers: 1,
            stars: 2,
            fetched_at: 90,
        };
        state
            .database
            .upsert_github_cache(&GithubCacheRow {
                username: cache_key("octocat"),
                payload_json: serde_json::to_string(&profile).expect("json"),
                fetched_at: 90,
            })
            .expect("g1 row");
        let view = github_view(&state, &runtime, 100).expect("view");
        assert!(view.snapshot.is_some());
        assert_eq!(view.search_cache_status, GithubCacheStatus::Missing);
        assert!(view.search.is_none());

        // G2 success + G1 failure/absence: the mirrored case. The search
        // snapshot renders while the profile plane reports missing.
        let state = configured_state("octocat");
        seed_search_cache(&state, &search_snapshot("octocat", 3, 5, 90));
        let view = github_view(&state, &runtime, 100).expect("view");
        assert_eq!(view.search.as_ref().map(|s| (s.open_issues, s.pull_requests)), Some((3, 5)));
        assert_eq!(view.cache_status, GithubCacheStatus::Missing);
        assert!(view.snapshot.is_none());
    }

    #[test]
    fn a_corrupt_or_mismatched_search_row_is_ignored_as_missing() {
        let state = configured_state("octocat");
        let runtime = GithubRuntime::default();
        state
            .database
            .upsert_github_search_cache(&GithubSearchCacheRow {
                username: cache_key("octocat"),
                payload_json: "corrupt".into(),
                fetched_at: 90,
            })
            .expect("corrupt search cache");
        assert!(
            github_view(&state, &runtime, 100).expect("view").search.is_none(),
            "a corrupt payload renders as missing, never as an error"
        );

        // A row whose payload names a different account is not this account's
        // snapshot, exactly like the profile plane's integrity filter.
        seed_search_cache(&state, &search_snapshot("someone-else", 3, 5, 90));
        let view = github_view(&state, &runtime, 100).expect("view");
        assert!(view.search.is_none());
    }

    #[test]
    fn a_username_change_starts_the_search_plane_from_an_empty_cache() {
        let state = configured_state("octocat");
        let runtime = GithubRuntime::default();
        seed_search_cache(&state, &search_snapshot("octocat", 3, 5, 90));

        // Same account, different spelling: the lowercase cache key reads the
        // same row.
        state
            .update(|settings| settings.github_username = "OctoCat".into())
            .expect("change spelling");
        let view = github_view(&state, &runtime, 100).expect("case-variant");
        assert!(view.search.is_some(), "case-insensitive search cache key");

        // A genuinely different account has nothing — its search totals must
        // never be borrowed from the previous account's row.
        state
            .update(|settings| settings.github_username = "someone-else".into())
            .expect("switch account");
        let view = github_view(&state, &runtime, 100).expect("new account");
        assert_eq!(view.search_cache_status, GithubCacheStatus::Missing);
        assert!(view.search.is_none());
    }

    #[test]
    fn the_view_exposes_the_search_plane_flags() {
        let state = configured_state("octocat");
        let runtime = GithubRuntime::default();
        let view = github_view(&state, &runtime, 100).expect("view");
        assert!(!view.search_refreshing);
        assert_eq!(view.search_last_refresh, "none");
        runtime.record_search("success");
        let view = github_view(&state, &runtime, 100).expect("view");
        assert_eq!(view.search_last_refresh, "success");
    }

    // --- G3: local-day boundaries ----------------------------------------------

    /// A timezone double that models a REAL transition landing on the
    /// wall-clock midnight of 2026-11-01, so the resolver's branches are
    /// driven on both the local→UTC and the UTC→local side without
    /// controlling the process timezone. `pre_seconds` is the offset before
    /// the transition and `gap_seconds` its signed shift (positive:
    /// spring-forward, which can skip midnight; negative: fall-back, which
    /// can repeat it). Every instant's offset is looked up against the
    /// modeled switch point, so a probe on the wrong side of the transition
    /// sees the wrong offset — the property a vacuous fixed-offset mock
    /// hides.
    #[derive(Clone)]
    struct EdgeTz {
        pre_seconds: i32,
        gap_seconds: i32,
        ambiguous_midnight: bool,
        skipped_midnight: bool,
    }

    impl EdgeTz {
        fn offset(&self, seconds: i32) -> chrono::FixedOffset {
            chrono::FixedOffset::east_opt(seconds).expect("valid offset")
        }
        fn pre(&self) -> chrono::FixedOffset {
            self.offset(self.pre_seconds)
        }
        fn post(&self) -> chrono::FixedOffset {
            self.offset(self.pre_seconds + self.gap_seconds)
        }
        fn midnight(&self) -> chrono::NaiveDateTime {
            chrono::NaiveDate::from_ymd_opt(2026, 11, 1)
                .expect("date")
                .and_hms_opt(0, 0, 0)
                .expect("midnight")
        }
        /// The UTC instant the offset switches at, when this mock models a
        /// midnight transition at all. A spring-forward's last pre-shift
        /// instant and a fall-back's repeated 00:00:00 sit one gap away from
        /// wall midnight on opposite sides, so each shape carries its own
        /// anchor.
        fn switch_utc(&self) -> Option<chrono::NaiveDateTime> {
            if self.skipped_midnight {
                Some(self.midnight() - self.pre())
            } else if self.ambiguous_midnight {
                Some(self.midnight() - self.post())
            } else {
                None
            }
        }
        fn offset_at_utc(&self, utc: &chrono::NaiveDateTime) -> chrono::FixedOffset {
            match self.switch_utc() {
                Some(switch) if *utc >= switch => self.post(),
                _ => self.pre(),
            }
        }
        /// The shared local→UTC branch: the edge behavior lives at exactly
        /// one wall-clock datetime; everything else resolves single at the
        /// pre-shift offset.
        fn from_local(&self, local: &chrono::NaiveDateTime) -> LocalResult<chrono::FixedOffset> {
            if *local == self.midnight() {
                if self.skipped_midnight {
                    return LocalResult::None;
                }
                if self.ambiguous_midnight {
                    // A fall-back: 00:00:00 exists twice — first at the
                    // pre-shift offset (the earlier instant), then again at
                    // the post-shift offset. chrono orders Ambiguous
                    // (earlier, later).
                    return LocalResult::Ambiguous(self.pre(), self.post());
                }
            }
            LocalResult::Single(self.pre())
        }
    }

    impl TimeZone for EdgeTz {
        type Offset = chrono::FixedOffset;

        fn from_offset(offset: &Self::Offset) -> Self {
            Self {
                pre_seconds: offset.local_minus_utc(),
                gap_seconds: 0,
                ambiguous_midnight: false,
                skipped_midnight: false,
            }
        }

        fn offset_from_utc_date(&self, utc: &chrono::NaiveDate) -> Self::Offset {
            self.offset_at_utc(&utc.and_hms_opt(0, 0, 0).expect("midnight"))
        }

        fn offset_from_utc_datetime(&self, utc: &chrono::NaiveDateTime) -> Self::Offset {
            self.offset_at_utc(utc)
        }

        fn offset_from_local_date(
            &self,
            local: &chrono::NaiveDate,
        ) -> LocalResult<Self::Offset> {
            self.from_local(&local.and_hms_opt(0, 0, 0).expect("midnight"))
        }

        fn offset_from_local_datetime(
            &self,
            local: &chrono::NaiveDateTime,
        ) -> LocalResult<Self::Offset> {
            self.from_local(local)
        }
    }

    /// Mid-day "now" inside the requested local day, fourteen UTC hours
    /// after the day's first pre-shift midnight occurrence — comfortably
    /// past any midnight transition, so the offset in force is the
    /// post-shift one whenever a transition was modeled.
    fn edge_now(tz: &EdgeTz) -> chrono::DateTime<EdgeTz> {
        let anchor = tz.midnight() - tz.pre() + chrono::Duration::hours(14);
        tz.from_utc_datetime(&anchor)
    }

    #[test]
    fn a_single_local_midnight_resolves_to_itself() {
        let tz = EdgeTz {
            pre_seconds: 8 * 3600,
            gap_seconds: 0,
            ambiguous_midnight: false,
            skipped_midnight: false,
        };
        let day_start = resolve_day_start(&tz, &edge_now(&tz));
        assert_eq!(day_start.naive_local(), tz.midnight());
        assert_eq!(day_start.offset().fix(), tz.pre().fix());
    }

    #[test]
    fn an_ambiguous_midnight_uses_the_first_occurrence() {
        // A fall-back: +03:00 becomes +02:00 at midnight, so 00:00:00
        // happens twice.
        let tz = EdgeTz {
            pre_seconds: 3 * 3600,
            gap_seconds: -3600,
            ambiguous_midnight: true,
            skipped_midnight: false,
        };
        let day_start = resolve_day_start(&tz, &edge_now(&tz));
        // The day begins at the FIRST 00:00:00: the pre-shift occurrence,
        // one pre-shift hour before wall midnight, a real instant of the
        // requested day.
        assert_eq!(
            day_start.with_timezone(&Utc).naive_utc(),
            tz.midnight() - tz.pre()
        );
        assert_eq!(day_start.naive_local(), tz.midnight());
        assert_eq!(day_start.offset().fix(), tz.pre().fix());
    }

    #[test]
    fn a_skipped_midnight_resolves_to_the_transition_instant() {
        // A spring-forward: +02:00 becomes +03:00 at midnight, so the
        // wall-clock 00:00:00 does not exist and the day's earliest instant
        // is the transition itself.
        let tz = EdgeTz {
            pre_seconds: 2 * 3600,
            gap_seconds: 3600,
            ambiguous_midnight: false,
            skipped_midnight: true,
        };
        let day_start = resolve_day_start(&tz, &edge_now(&tz));
        // The resolved instant IS the switch point, and its local wall time
        // is midnight plus the gap (01:00:00) — genuinely inside the
        // requested local day. A resolver that merely reinterprets wall
        // midnight at the offset in force now lands one gap-hour early, in
        // the previous local day, and fails both assertions.
        assert_eq!(
            day_start.with_timezone(&Utc).naive_utc(),
            tz.midnight() - tz.pre()
        );
        assert_eq!(
            day_start.naive_local(),
            tz.midnight() + chrono::Duration::seconds(tz.gap_seconds as i64)
        );
        assert_eq!(day_start.offset().fix(), tz.post().fix());
    }

    #[test]
    fn the_local_day_context_names_the_day_and_its_midnight_stably() {
        let now = 1_800_000_000;
        let (day, day_start) = local_day_context(now);
        // The day key is the midnight's own calendar date, and the midnight is
        // a real local 00:00:00.
        assert_eq!(day, {
            let midnight = Local
                .timestamp_opt(day_start, 0)
                .single()
                .expect("the day start is a real instant");
            assert_eq!(midnight.format("%H:%M:%S").to_string(), "00:00:00");
            midnight.date_naive().format("%Y-%m-%d").to_string()
        });
        // The midnight precedes the instant, on the same local day (a DST day
        // can be 23 or 25 hours, so the bound is loose).
        assert!(day_start <= now);
        assert!(now - day_start < 25 * 3600);
        // The pair is a deterministic function of the calendar day: asked
        // again later the same day, both answers agree.
        let (later_day, later_start) = local_day_context(now + 600);
        assert_eq!((day, day_start), (later_day, later_start));
    }

    // --- G3: query construction --------------------------------------------------

    #[test]
    fn the_commits_query_bounds_local_midnight_and_the_fetch_instant() {
        let fetched_at = 1_800_000_000;
        let day_start = local_datetime_at(fetched_at).expect("local datetime");
        let query = commits_query("octo-cat", &day_start, fetched_at);
        let start_text = resolve_day_start(&Local, &day_start)
            .format(COMMITS_TIME_FORMAT)
            .to_string();
        let end_text = day_start
            .fixed_offset()
            .format(COMMITS_TIME_FORMAT)
            .to_string();
        assert_eq!(
            query,
            format!("author:octo-cat is:public author-date:{start_text}..{end_text}"),
            "the range starts at local midnight (with that boundary's own offset) and ends exactly at the fetch instant"
        );
        // The upper bound is the fetch instant: a commit whose author
        // timestamp falls after the fetch time is outside the range and can
        // never be counted by this query.
        assert!(end_text.contains(&Local
            .timestamp_opt(fetched_at, 0)
            .single()
            .expect("real instant")
            .format("%Y-%m-%dT%H:%M:%S")
            .to_string()));
    }

    #[test]
    fn the_commits_url_encodes_the_query_under_the_fixed_api_origin() {
        let url = commits_search_url("octo-cat", 1_800_000_000).expect("commits url");
        let text = url.as_str();
        assert!(text.starts_with("https://api.github.com/search/commits?q="), "{text}");
        assert!(text.contains("author%3Aocto-cat"), "{text}");
        assert!(text.contains("is%3Apublic"), "{text}");
        assert!(text.contains("author-date%3A"), "{text}");
        assert!(text.contains("%2B"), "the offset's plus sign is URL-encoded: {text}");
        assert!(text.contains(".."), "the range separator survives encoding: {text}");
        assert!(text.ends_with("&per_page=1"), "{text}");
    }

    // --- G3: fetch ---------------------------------------------------------------

    fn commits_response(total_count: u64, incomplete_results: bool) -> HttpResponse {
        profile_response(200, &search_body(total_count, incomplete_results))
    }

    #[test]
    fn the_commits_fetch_reads_one_bounded_search_request() {
        let (day, day_start) = local_day_context(100);
        let transport = QueueTransport::new(vec![commits_response(7, false)]);
        let snapshot = fetch_commits_with_transport(
            &SearchCoordinator::default(),
            &transport,
            "octocat",
            100,
        )
        .expect("snapshot");
        assert_eq!(snapshot.commits, 7, "total_count is read verbatim");
        assert_eq!(snapshot.username, "octocat");
        assert_eq!(snapshot.local_day, day);
        assert_eq!(snapshot.local_day_start, day_start);
        assert_eq!(snapshot.fetched_at, 100);
        assert_eq!(
            transport.calls.load(AtomicOrdering::Relaxed),
            1,
            "the request budget is exactly one search request"
        );
        assert!(transport.urls()[0].contains("/search/commits"), "{:?}", transport.urls());
    }

    #[test]
    fn a_zero_commit_day_is_genuine_data_not_missing_data() {
        let transport = QueueTransport::new(vec![commits_response(0, false)]);
        let snapshot = fetch_commits_with_transport(
            &SearchCoordinator::default(),
            &transport,
            "octocat",
            100,
        )
        .expect("snapshot");
        assert_eq!(snapshot.commits, 0);
    }

    #[test]
    fn an_incomplete_commits_answer_is_refused_never_counted() {
        let transport = QueueTransport::new(vec![commits_response(1000, true)]);
        assert_eq!(
            fetch_commits_with_transport(
                &SearchCoordinator::default(),
                &transport,
                "octocat",
                100
            ),
            Err(GithubError::Incomplete),
            "incomplete_results=true is never presented as an exact count"
        );
        assert_eq!(
            transport.calls.load(AtomicOrdering::Relaxed),
            1,
            "the refusal happens on the one request; no second request exists"
        );
    }

    #[test]
    fn commits_refusals_map_to_the_pinned_vocabulary() {
        // 429 → RateLimited with the provider's hint.
        let mut too_many = status_response(429);
        too_many.retry_after = Some("30".into());
        let transport = QueueTransport::new(vec![too_many]);
        assert_eq!(
            fetch_commits_with_transport(
                &SearchCoordinator::default(),
                &transport,
                "octocat",
                100
            ),
            Err(GithubError::RateLimited(Some(30)))
        );

        // 403 with the remaining budget at zero → RateLimited.
        let mut exhausted = status_response(403);
        exhausted.rate_limit_remaining = Some("0".into());
        exhausted.rate_limit_reset = Some("250".into());
        let transport = QueueTransport::new(vec![exhausted]);
        assert_eq!(
            fetch_commits_with_transport(
                &SearchCoordinator::default(),
                &transport,
                "octocat",
                100
            ),
            Err(GithubError::RateLimited(Some(150)))
        );

        // 403 with only a Retry-After — the secondary rate-limit signature —
        // is a rate limit, not a generic refusal.
        let mut secondary = status_response(403);
        secondary.retry_after = Some("60".into());
        let transport = QueueTransport::new(vec![secondary]);
        assert_eq!(
            fetch_commits_with_transport(
                &SearchCoordinator::default(),
                &transport,
                "octocat",
                100
            ),
            Err(GithubError::RateLimited(Some(60)))
        );

        // 403 without any rate-limit shape, and 422/404/500, stay generic
        // provider errors — the account-existence fact belongs to the profile
        // plane, never to a search plane.
        for status in [403, 422, 404, 500] {
            let transport = QueueTransport::new(vec![status_response(status)]);
            assert_eq!(
                fetch_commits_with_transport(
                    &SearchCoordinator::default(),
                    &transport,
                    "octocat",
                    100
                ),
                Err(GithubError::ProviderError),
                "status {status} must be a provider error"
            );
        }
    }

    #[test]
    fn a_malformed_commits_answer_is_an_invalid_response() {
        let transport = QueueTransport::new(vec![profile_response(200, "not json")]);
        assert_eq!(
            fetch_commits_with_transport(
                &SearchCoordinator::default(),
                &transport,
                "octocat",
                100
            ),
            Err(GithubError::InvalidResponse)
        );
        // A missing aggregate field is malformed too.
        let transport = QueueTransport::new(vec![profile_response(200, r#"{ "total_count": 5 }"#)]);
        assert_eq!(
            fetch_commits_with_transport(
                &SearchCoordinator::default(),
                &transport,
                "octocat",
                100
            ),
            Err(GithubError::InvalidResponse)
        );
    }

    // --- G3: shared search-request coordination ----------------------------------

    #[test]
    fn the_search_window_admits_the_documented_budget_then_refuses() {
        let coordinator = SearchCoordinator::default();
        let now = Instant::now();
        for _ in 0..SEARCH_REQUESTS_PER_WINDOW {
            coordinator.request_permit(now).expect("within budget");
        }
        let refused = coordinator.request_permit(now).expect_err("over budget");
        match refused {
            GithubError::RateLimited(Some(seconds)) => {
                assert!(
                    (1..=60).contains(&seconds),
                    "the hint is the real window remainder, got {seconds}s"
                );
            }
            other => panic!("a full window must refuse as RateLimited, got {other:?}"),
        }
        // Sliding the window forward frees every slot again.
        let later = now + SEARCH_WINDOW;
        coordinator.request_permit(later).expect("window slid");
    }

    #[test]
    fn a_refused_search_request_records_nothing_new() {
        let coordinator = SearchCoordinator::default();
        let now = Instant::now();
        for _ in 0..SEARCH_REQUESTS_PER_WINDOW {
            coordinator.request_permit(now).expect("within budget");
        }
        assert!(coordinator.request_permit(now).is_err());
        // The refusal consumed no slot: the window still holds exactly the
        // ten admitted dispatches.
        assert_eq!(
            coordinator.window.lock().expect("window").len(),
            SEARCH_REQUESTS_PER_WINDOW
        );
    }

    /// A transport that records its peak in-flight request count — the
    /// serialization witness for the shared search coordinator.
    struct ConcurrencyTransport {
        current: AtomicUsize,
        peak: AtomicUsize,
    }

    impl ConcurrencyTransport {
        fn new() -> Self {
            Self {
                current: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
            }
        }

        fn peak(&self) -> usize {
            self.peak.load(AtomicOrdering::Relaxed)
        }
    }

    impl HttpTransport for ConcurrencyTransport {
        fn get(&self, _url: Url) -> Result<HttpResponse, GithubError> {
            let current = self.current.fetch_add(1, AtomicOrdering::Relaxed) + 1;
            self.peak.fetch_max(current, AtomicOrdering::Relaxed);
            std::thread::sleep(Duration::from_millis(20));
            self.current.fetch_sub(1, AtomicOrdering::Relaxed);
            Ok(commits_response(1, false))
        }
    }

    #[test]
    fn guarded_search_requests_never_fly_concurrently() {
        let coordinator = std::sync::Arc::new(SearchCoordinator::default());
        let transport = std::sync::Arc::new(ConcurrencyTransport::new());
        let url = search_issues_url("octocat").expect("url");
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let coordinator = std::sync::Arc::clone(&coordinator);
                let transport = std::sync::Arc::clone(&transport);
                let url = url.clone();
                std::thread::spawn(move || coordinator.guarded_get(transport.as_ref(), url))
            })
            .collect();
        for handle in handles {
            handle.join().expect("thread").expect("request");
        }
        assert_eq!(
            transport.peak(),
            1,
            "search requests serialize: at most one is in flight at any time"
        );
    }

    #[test]
    fn the_search_budget_is_shared_across_both_search_planes() {
        let coordinator = SearchCoordinator::default();
        // Five G2 fetches spend exactly ten search requests (two each).
        for _ in 0..(SEARCH_REQUESTS_PER_WINDOW / 2) {
            let transport = QueueTransport::new(vec![
                search_response(1, false),
                search_response(1, false),
            ]);
            fetch_search_totals_with_transport(&coordinator, &transport, "octocat", 100)
                .expect("within budget");
        }
        // The commits plane draws on the same window: one more request is
        // refused before the transport is touched.
        let transport = QueueTransport::new(vec![commits_response(1, false)]);
        let refused = fetch_commits_with_transport(&coordinator, &transport, "octocat", 100)
            .expect_err("over budget");
        assert!(
            matches!(refused, GithubError::RateLimited(Some(_))),
            "the shared window refuses cross-plane too, got {refused:?}"
        );
        assert_eq!(
            transport.calls.load(AtomicOrdering::Relaxed),
            0,
            "a locally refused search request never reaches the network"
        );
    }

    #[test]
    fn a_refused_second_g2_request_fails_the_whole_search_refresh() {
        // Nine dispatches already in the window: G2's first request is
        // admitted, its second is refused — and the fetch fails whole, so
        // nothing partial is ever presented as a snapshot.
        let coordinator = SearchCoordinator::default();
        let now = Instant::now();
        for _ in 0..(SEARCH_REQUESTS_PER_WINDOW - 1) {
            coordinator.request_permit(now).expect("within budget");
        }
        let transport = QueueTransport::new(vec![
            search_response(1, false),
            search_response(1, false),
        ]);
        let error =
            fetch_search_totals_with_transport(&coordinator, &transport, "octocat", 100)
                .expect_err("over budget");
        assert!(
            matches!(error, GithubError::RateLimited(Some(_))),
            "the mid-fetch refusal is a rate limit, got {error:?}"
        );
        assert_eq!(
            transport.calls.load(AtomicOrdering::Relaxed),
            1,
            "the first request flew, the second was refused locally"
        );
    }

    // --- G3: runtime gates -------------------------------------------------------

    #[test]
    fn commits_refreshes_deduplicate_and_cool_down_independently() {
        let runtime = GithubRuntime::default();
        runtime.begin_commits_refresh(false).expect("first");
        assert_eq!(
            runtime.begin_commits_refresh(false),
            Err(GithubError::InProgress),
            "a concurrent identical commits fetch is deduplicated"
        );
        runtime.finish_commits_refresh();
        // The planes' single-flight flags are independent: the commits
        // plane's flight never blocks either other plane.
        runtime.begin_refresh(false).expect("profile plane unaffected");
        runtime.finish_refresh();
        runtime.begin_search_refresh(false).expect("search plane unaffected");
        runtime.finish_search_refresh();

        runtime.begin_commits_refresh(true).expect("manual");
        runtime.finish_commits_refresh();
        assert_eq!(
            runtime.begin_commits_refresh(true),
            Err(GithubError::Cooldown),
            "a manual commits refresh inside the cooldown is refused"
        );
        // The cooldown clocks are independent too.
        runtime.begin_refresh(true).expect("profile cooldown independent");
        runtime.finish_refresh();
        runtime.begin_search_refresh(true).expect("search cooldown independent");
        runtime.finish_search_refresh();
        // Background commits refreshes are not cooled down.
        runtime.begin_commits_refresh(false).expect("background");
        runtime.finish_commits_refresh();
    }

    #[test]
    fn a_rate_limit_backoff_is_shared_by_all_three_planes() {
        let runtime = GithubRuntime::default();
        runtime.note_rate_limit(Some(120));
        let remaining = runtime.rate_limit_remaining().expect("backoff in force");
        assert_eq!(
            runtime.begin_commits_refresh(false),
            Err(GithubError::RateLimited(Some(remaining))),
            "the commits plane refuses during the shared backoff"
        );
        assert_eq!(
            runtime.begin_search_refresh(false),
            Err(GithubError::RateLimited(Some(remaining))),
            "so does the search plane"
        );
        assert_eq!(
            runtime.begin_refresh(false),
            Err(GithubError::RateLimited(Some(remaining))),
            "so does the profile plane"
        );
    }

    // --- G3: commits cache and view ----------------------------------------------

    /// Seeds a commits row valid for `view_now`'s local day and midnight, so
    /// the test's expectation and the view's recomputation share one clock
    /// reading and cannot drift across a real midnight.
    fn seed_commits_cache(state: &AppState, username: &str, commits: u64, view_now: i64, age: i64) {
        let (local_day, local_day_start) = local_day_context(view_now);
        let snapshot = CommitsSnapshot {
            username: username.into(),
            commits,
            local_day,
            local_day_start,
            fetched_at: view_now - age,
        };
        state
            .database
            .upsert_github_commits_cache(&GithubCommitsCacheRow {
                username: cache_key(username),
                payload_json: serde_json::to_string(&snapshot).expect("json"),
                fetched_at: snapshot.fetched_at,
            })
            .expect("seed commits cache");
    }

    #[test]
    fn commits_cache_reports_missing_fresh_and_stale() {
        let state = configured_state("octocat");
        let runtime = GithubRuntime::default();
        assert_eq!(
            github_view(&state, &runtime, 100_000)
                .expect("missing")
                .commits_cache_status,
            GithubCacheStatus::Missing
        );
        for (age, expected) in [
            (60, GithubCacheStatus::Fresh),
            (FRESH_AFTER_SECONDS, GithubCacheStatus::Stale),
            (31 * 60, GithubCacheStatus::Stale),
        ] {
            seed_commits_cache(&state, "octocat", 4, 100_000, age);
            let view = github_view(&state, &runtime, 100_000).expect("view");
            assert_eq!(view.commits_cache_status, expected, "age {age}");
            assert_eq!(view.commits_cache_age_seconds, Some(age));
            assert_eq!(view.commits.as_ref().map(|s| s.commits), Some(4));
        }
    }

    #[test]
    fn a_snapshot_from_another_day_or_moved_midnight_renders_missing() {
        let state = configured_state("octocat");
        let runtime = GithubRuntime::default();
        let (today, today_start) = local_day_context(100_000);

        // Yesterday's snapshot: fresh by age, but it answers the wrong
        // question. It is refused whole — yesterday's number is never
        // rendered as today's count.
        let yesterday = CommitsSnapshot {
            username: "octocat".into(),
            commits: 9,
            local_day: "2000-01-01".into(),
            local_day_start: 0,
            fetched_at: 100_000 - 60,
        };
        state
            .database
            .upsert_github_commits_cache(&GithubCommitsCacheRow {
                username: cache_key("octocat"),
                payload_json: serde_json::to_string(&yesterday).expect("json"),
                fetched_at: yesterday.fetched_at,
            })
            .expect("seed yesterday");
        let view = github_view(&state, &runtime, 100_000).expect("view");
        assert_eq!(view.commits_cache_status, GithubCacheStatus::Missing);
        assert!(view.commits.is_none());

        // The timezone-move case: the same calendar date spelled the same
        // way, but computed against a different midnight. The day string
        // alone cannot distinguish the two contexts, so the exact midnight
        // instant is part of the identity too.
        let moved = CommitsSnapshot {
            username: "octocat".into(),
            commits: 9,
            local_day: today,
            local_day_start: today_start + 1,
            fetched_at: 100_000 - 60,
        };
        state
            .database
            .upsert_github_commits_cache(&GithubCommitsCacheRow {
                username: cache_key("octocat"),
                payload_json: serde_json::to_string(&moved).expect("json"),
                fetched_at: moved.fetched_at,
            })
            .expect("seed moved");
        let view = github_view(&state, &runtime, 100_000).expect("view");
        assert_eq!(view.commits_cache_status, GithubCacheStatus::Missing);
        assert!(view.commits.is_none());
    }

    #[test]
    fn a_corrupt_or_mismatched_commits_row_is_ignored_as_missing() {
        let state = configured_state("octocat");
        let runtime = GithubRuntime::default();
        state
            .database
            .upsert_github_commits_cache(&GithubCommitsCacheRow {
                username: cache_key("octocat"),
                payload_json: "corrupt".into(),
                fetched_at: 90,
            })
            .expect("corrupt commits cache");
        assert!(
            github_view(&state, &runtime, 100_000)
                .expect("view")
                .commits
                .is_none(),
            "a corrupt payload renders as missing, never as an error"
        );

        // A row whose payload names a different account is not this
        // account's snapshot, exactly like the other planes' integrity
        // filters.
        seed_commits_cache(&state, "someone-else", 4, 100_000, 60);
        let view = github_view(&state, &runtime, 100_000).expect("view");
        assert!(view.commits.is_none());
    }

    #[test]
    fn the_three_cache_planes_are_independent() {
        let state = configured_state("octocat");
        let runtime = GithubRuntime::default();

        // A fresh commits snapshot renders while both other planes report
        // missing.
        seed_commits_cache(&state, "octocat", 4, 100_000, 60);
        let view = github_view(&state, &runtime, 100_000).expect("view");
        assert_eq!(view.commits.as_ref().map(|s| s.commits), Some(4));
        assert_eq!(view.commits_cache_status, GithubCacheStatus::Fresh);
        assert_eq!(view.cache_status, GithubCacheStatus::Missing);
        assert!(view.snapshot.is_none());
        assert_eq!(view.search_cache_status, GithubCacheStatus::Missing);
        assert!(view.search.is_none());

        // The mirrored case: valid profile and search data with no commits
        // row at all — the commits plane reports missing and the others stay
        // untouched.
        let state = configured_state("octocat");
        let profile = GithubSnapshot {
            username: "octocat".into(),
            followers: 1,
            stars: 2,
            fetched_at: 90,
        };
        state
            .database
            .upsert_github_cache(&GithubCacheRow {
                username: cache_key("octocat"),
                payload_json: serde_json::to_string(&profile).expect("json"),
                fetched_at: 90,
            })
            .expect("g1 row");
        seed_search_cache(&state, &search_snapshot("octocat", 3, 5, 90));
        let view = github_view(&state, &runtime, 100_000).expect("view");
        assert!(view.snapshot.is_some());
        assert!(view.search.is_some());
        assert_eq!(view.commits_cache_status, GithubCacheStatus::Missing);
        assert!(view.commits.is_none());
    }

    #[test]
    fn a_username_change_starts_the_commits_plane_from_an_empty_cache() {
        let state = configured_state("octocat");
        let runtime = GithubRuntime::default();
        seed_commits_cache(&state, "octocat", 4, 100_000, 60);

        // Same account, different spelling: the lowercase cache key reads
        // the same row.
        state
            .update(|settings| settings.github_username = "OctoCat".into())
            .expect("change spelling");
        let view = github_view(&state, &runtime, 100_000).expect("case-variant");
        assert!(view.commits.is_some(), "case-insensitive commits cache key");

        // A genuinely different account has nothing — its commits must never
        // be borrowed from the previous account's row.
        state
            .update(|settings| settings.github_username = "someone-else".into())
            .expect("switch account");
        let view = github_view(&state, &runtime, 100_000).expect("new account");
        assert_eq!(view.commits_cache_status, GithubCacheStatus::Missing);
        assert!(view.commits.is_none());
    }

    #[test]
    fn the_view_exposes_the_commits_plane_flags() {
        let state = configured_state("octocat");
        let runtime = GithubRuntime::default();
        let view = github_view(&state, &runtime, 100_000).expect("view");
        assert!(!view.commits_refreshing);
        assert_eq!(view.commits_last_refresh, "none");
        runtime.record_commits("success");
        let view = github_view(&state, &runtime, 100_000).expect("view");
        assert_eq!(view.commits_last_refresh, "success");
    }
}
