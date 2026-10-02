//! Bounded HTTP transport for updater discovery.
//!
//! This is the only place the updater touches HTTP. Two frozen rules shape
//! it: the signature covers the exact raw bytes as served (so nothing here
//! may trim, decode, transcode, or normalize a body — see the reqwest
//! feature note in Cargo.toml), and "redirects and download hosts need
//! adapter allowlists" with network timeouts and response-size limits
//! (application-lifecycle §7). Every duration/count value below is
//! implementation transport policy, not a signed protocol semantic rule.

use std::collections::HashSet;
use std::io::Read;
use std::time::Duration;

use crate::updater::DiscoveryConfig;

/// Which bounded document a fetch carries. Diagnostics only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    ProviderIndex,
    Envelope,
    Manifest,
}

/// Transport-layer failures of a provider HTTP call. These are never
/// candidate verdicts — a candidate only reaches the verifier after a
/// complete bounded body is on hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// Connection/IO/request failure that is not a timeout.
    Transport { detail: String },
    /// Bounded finite timeout expired.
    Timeout { detail: String },
    /// The provider answered outside the accepted status set.
    HttpStatus {
        status: u16,
        retry_after: Option<Duration>,
    },
    /// The provider answered within the accepted status set, but the body is
    /// not the expected JSON shape — the discovery layer is malfunctioning.
    /// Distinct from a transport failure, and never retried (deterministic).
    ResponseMalformed { detail: String },
    /// Reading the body failed mid-stream.
    BodyRead { detail: String },
    /// The served representation carries a content encoding other than
    /// identity, so the transport bytes cannot be the signed-byte identity
    /// the verifier requires. Transport implementation policy, fail closed;
    /// never a cryptographic verdict.
    ContentEncodingRejected { encoding: String },
    /// The body exceeded its bound. Detected before the full body arrives:
    /// either a declared Content-Length over the cap, or the stream read
    /// hitting the hard `cap + 1` ceiling.
    BodyTooLarge { kind: BodyKind, cap: usize },
    /// A redirect left the compiled origin allowlist or exceeded the bound.
    RedirectRejected { detail: String },
    /// The whole-scan budget expired before this call could start or finish.
    /// Client-side stop: never a candidate verdict and never a
    /// provider-layer failure.
    BudgetExhausted,
}

impl FetchError {
    /// The transient failures the bounded retry ladder covers. The frozen
    /// text names timeouts and rate limits explicitly; other server-side
    /// availability statuses (500/502/504) join them as implementation
    /// transport policy. Everything else is final.
    pub(crate) fn is_transient(&self) -> bool {
        matches!(
            self,
            FetchError::Timeout { .. }
                | FetchError::Transport { .. }
                | FetchError::HttpStatus {
                    status: 429 | 500 | 502 | 503 | 504,
                    ..
                }
        )
    }

    /// Budget expiry is a client-side stop, never a retryable provider
    /// condition.
    pub(crate) fn is_budget_exhausted(&self) -> bool {
        matches!(self, FetchError::BudgetExhausted)
    }
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Transport { detail } => write!(f, "transport failure: {detail}"),
            FetchError::Timeout { detail } => write!(f, "bounded timeout: {detail}"),
            FetchError::HttpStatus {
                status,
                retry_after,
            } => {
                write!(f, "provider HTTP status {status}")?;
                if let Some(delay) = retry_after {
                    write!(f, " (Retry-After {delay:?})")
                } else {
                    Ok(())
                }
            }
            FetchError::ResponseMalformed { detail } => {
                write!(f, "provider response malformed: {detail}")
            }
            FetchError::BodyRead { detail } => write!(f, "body read failure: {detail}"),
            FetchError::BodyTooLarge { kind, cap } => {
                write!(f, "{kind:?} body exceeded the {cap}-byte bound")
            }
            FetchError::ContentEncodingRejected { encoding } => {
                write!(f, "non-identity content encoding {encoding:?} rejected")
            }
            FetchError::RedirectRejected { detail } => {
                write!(f, "redirect rejected: {detail}")
            }
            FetchError::BudgetExhausted => {
                write!(f, "whole-scan budget expired")
            }
        }
    }
}

/// User agent for all updater discovery requests (GitHub's API requires one).
pub(crate) const USER_AGENT: &str = "desktop-todo-widget-updater";

/// Origin (`scheme://host[:port]`) of a URL, lowercase host, explicit port
/// kept, default port omitted. The unit the redirect allowlist speaks.
fn origin(url: &reqwest::Url) -> Option<String> {
    let scheme = url.scheme();
    let host = url.host_str()?;
    let port = match url.port() {
        Some(port) => format!(":{port}"),
        None => String::new(),
    };
    Some(format!("{scheme}://{host}{port}").to_ascii_lowercase())
}

/// Pure redirect decision, testable without a network: only origins on the
/// adapter allowlist may be followed, so a provider can never bounce signed
/// metadata to an arbitrary host, and no https→http downgrade exists because
/// every allowlisted origin is https in production.
fn redirect_allowed(url: &reqwest::Url, allowlist: &HashSet<String>) -> bool {
    origin(url).is_some_and(|o| allowlist.contains(&o))
}

/// Build the shared discovery client: finite timeouts, no automatic content
/// decoding (no compression features), and a redirect policy restricted to
/// the compiled origin allowlist. Crate-internal — external code can never
/// obtain an arbitrary-endpoint fetch capability from this crate; tests
/// construct it against injected mock endpoints.
pub(crate) fn discovery_client(
    config: &DiscoveryConfig,
    allowed_origins: &[String],
) -> Result<reqwest::blocking::Client, FetchError> {
    let allowlist: HashSet<String> = allowed_origins
        .iter()
        .map(|o| o.to_ascii_lowercase())
        .collect();
    let max_redirects = config.max_redirects;
    let client = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(config.connect_timeout)
        .timeout(config.request_timeout)
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= max_redirects {
                attempt.error("redirect count exceeded the discovery bound")
            } else if redirect_allowed(attempt.url(), &allowlist) {
                attempt.follow()
            } else {
                attempt.error("redirect target is outside the adapter origin allowlist")
            }
        }))
        .build()
        .map_err(|e| FetchError::Transport {
            detail: format!("discovery client build failed: {e}"),
        })?;
    Ok(client)
}

fn map_send_error(error: reqwest::Error) -> FetchError {
    if error.is_redirect() {
        FetchError::RedirectRejected {
            detail: error.to_string(),
        }
    } else if error.is_timeout() {
        FetchError::Timeout {
            detail: error.to_string(),
        }
    } else if error.is_body() || error.is_decode() {
        FetchError::BodyRead {
            detail: error.to_string(),
        }
    } else {
        FetchError::Transport {
            detail: error.to_string(),
        }
    }
}

/// GET `url` and return the exact transport bytes, bounded before the full
/// body is read: a present Content-Length over the cap rejects up front, and
/// the stream read is hard-capped at `cap + 1` bytes so chunked or lying
/// Content-Length responses can never drive unbounded memory growth. The
/// returned bytes are the served representation bytes, unmodified.
pub(crate) fn fetch_bounded(
    client: &reqwest::blocking::Client,
    url: &str,
    cap: usize,
    kind: BodyKind,
    per_request_timeout: Duration,
) -> Result<Vec<u8>, FetchError> {
    let response = client
        .get(url)
        // Ask for the unencoded representation explicitly: the signature
        // covers the exact served bytes, so no content transformation may
        // stand between the source and the verifier.
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        // Per-request timeout bounded by the remaining whole-scan budget —
        // a single 30s request can never outlive a 300ms budget.
        .timeout(per_request_timeout)
        .send()
        .map_err(map_send_error)?;
    // Fail closed on any non-identity content encoding (header absent or
    // `identity` pass): a gzip/br/deflate/zstd/unknown representation would
    // not be the byte identity the signature was computed over. Transport
    // implementation policy, never a cryptographic verdict.
    if let Some(encoding) = response.headers().get(reqwest::header::CONTENT_ENCODING) {
        let encoding = encoding.to_str().unwrap_or_default().trim();
        if !encoding.eq_ignore_ascii_case("identity") {
            return Err(FetchError::ContentEncodingRejected {
                encoding: encoding.to_string(),
            });
        }
    }
    let status = response.status();
    if !status.is_success() {
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        return Err(FetchError::HttpStatus {
            status: status.as_u16(),
            retry_after,
        });
    }
    // Early bound when the server declares one — never trusted alone.
    if let Some(len) = response.content_length() {
        if len > cap as u64 {
            return Err(FetchError::BodyTooLarge { kind, cap });
        }
    }
    // Hard read ceiling of cap + 1: the read stops there even when the body
    // is chunked or the declared length lies.
    let mut reader = response.take(cap as u64 + 1);
    let mut body = Vec::new();
    reader
        .read_to_end(&mut body)
        .map_err(|e| FetchError::BodyRead {
            detail: e.to_string(),
        })?;
    if body.len() > cap {
        return Err(FetchError::BodyTooLarge { kind, cap });
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_decision_is_origin_exact_and_https_scoped() {
        let allowlist: HashSet<String> = [
            "https://github.com",
            "https://objects.githubusercontent.com",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let ok = reqwest::Url::parse("https://github.com/o/r/releases/download/v1/f").unwrap();
        let cdn = reqwest::Url::parse("https://objects.githubusercontent.com/x").unwrap();
        let evil = reqwest::Url::parse("https://evil.example/x").unwrap();
        let downgrade = reqwest::Url::parse("http://github.com/x").unwrap();
        let subdomain = reqwest::Url::parse("https://api.github.com/x").unwrap();
        assert!(redirect_allowed(&ok, &allowlist));
        assert!(redirect_allowed(&cdn, &allowlist));
        assert!(!redirect_allowed(&evil, &allowlist));
        // Scheme downgrade is never on the allowlist: production origins are
        // all https, and http://github.com is a different origin.
        assert!(!redirect_allowed(&downgrade, &allowlist));
        // Sibling hosts of an allowlisted host are not implied.
        assert!(!redirect_allowed(&subdomain, &allowlist));
    }

    #[test]
    fn origin_keeps_explicit_ports_and_omits_default_ports() {
        let url = reqwest::Url::parse("http://127.0.0.1:41234/x").unwrap();
        assert_eq!(origin(&url).as_deref(), Some("http://127.0.0.1:41234"));
        let url = reqwest::Url::parse("https://github.com/x").unwrap();
        assert_eq!(origin(&url).as_deref(), Some("https://github.com"));
    }

    /// Phase 4B: the Gitee allowlist admits the observed attachment CDN as
    /// one exact origin. The decision stays a scheme+host+port equality on
    /// the canonicalized origin — no wildcard, no suffix trust, no sibling
    /// or look-alike host, no scheme downgrade, no non-default port.
    #[test]
    fn gitee_origin_matrix_admits_only_the_two_compiled_origins() {
        let allowlist: HashSet<String> = ["https://gitee.com", "https://foruda.gitee.com"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let allowed = |url: &str| {
            redirect_allowed(&reqwest::Url::parse(url).unwrap(), &allowlist)
        };
        // Both compiled origins allow paths of any shape.
        assert!(allowed("https://gitee.com/alanfloyd-dev/desktop-todo-widget/releases/download/v1.1.0/f"));
        assert!(allowed("https://gitee.com/alanfloyd-dev/desktop-todo-widget/attach_files/3308927/download/f"));
        assert!(allowed("https://foruda.gitee.com/attach_file/1790954961346017399/f?token=x&ts=1"));
        // Scheme downgrade: a different origin, refused.
        assert!(!allowed("http://foruda.gitee.com/attach_file/1/f"));
        assert!(!allowed("http://gitee.com/x"));
        // Suffix / sibling / look-alike hosts: exact match only.
        assert!(!allowed("https://foruda.gitee.com.evil.example/f"));
        assert!(!allowed("https://evilforuda.gitee.com/f"));
        assert!(!allowed("https://sub.foruda.gitee.com/f"));
        assert!(!allowed("https://gitee.com.evil.example/f"));
        // A non-default port is a different origin; the default port
        // canonicalizes away, so an explicit :443 stays equivalent.
        assert!(!allowed("https://foruda.gitee.com:444/attach_file/1/f"));
        assert!(allowed("https://foruda.gitee.com:443/attach_file/1/f"));
        // Userinfo is not part of a URL origin (scheme+host+port), so
        // https://user@foruda.gitee.com is the same origin as its host —
        // the URL-standard behavior of reqwest::Url, asserted here rather
        // than invented. It grants nothing beyond that exact host.
        assert!(allowed("https://user@foruda.gitee.com/f"));
        assert!(!allowed("https://user@evil.example/f"));
        // The path/query never widens the decision.
        assert!(!allowed("https://foruda.gitee.com.evil.example/https://gitee.com/"));
    }
}
