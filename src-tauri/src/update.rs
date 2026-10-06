//! Startup check for a newer release on GitHub.
//!
//! One unauthenticated GET to the GitHub API's "latest release" endpoint, only when the user has
//! the setting on. Nothing about the user or their llama-swap is sent. Only the release tag is
//! read from the response; the page that gets opened is built here from that tag, never taken
//! from the response.

use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};

pub const GITHUB_API: &str = "https://api.github.com";
pub const REPO: &str = "anphonic/llama-swatch";
const TIMEOUT: Duration = Duration::from_secs(10);
/// A release response is a few KB; anything far larger is not one.
const MAX_BODY: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    /// The release tag, e.g. `v0.2.0`.
    pub tag: String,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
}

/// `X.Y.Z` with plain decimal parts. Pre-release or build suffixes are rejected.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let mut parts = s.split('.');
    let mut next = || -> Option<u64> {
        let p = parts.next()?;
        if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        p.parse().ok()
    };
    let v = (next()?, next()?, next()?);
    parts.next().is_none().then_some(v)
}

/// A release tag: `v` followed by a strict `X.Y.Z`.
pub fn parse_tag(tag: &str) -> Option<(u64, u64, u64)> {
    parse_version(tag.strip_prefix('v')?)
}

/// The GitHub page for a release tag, or `None` if the tag is not a strict `vX.Y.Z`.
pub fn release_url(tag: &str) -> Option<String> {
    parse_tag(tag).map(|_| format!("https://github.com/{REPO}/releases/tag/{tag}"))
}

/// `Some` when `latest_tag` is a valid tag newer than `current` (a `CARGO_PKG_VERSION`).
pub fn newer(latest_tag: &str, current: &str) -> Option<UpdateInfo> {
    let latest = parse_tag(latest_tag)?;
    let current = parse_version(current)?;
    (latest > current).then(|| UpdateInfo { tag: latest_tag.to_string() })
}

/// Asks `api_base` (the GitHub API, or a test server) for the latest release of `REPO`.
/// `Ok(None)` when there is no newer release, or no release at all.
pub async fn check(api_base: &str, current: &str) -> Result<Option<UpdateInfo>, String> {
    let http = Client::builder()
        .timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("llama-swatch/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.without_url().to_string())?;
    let url = format!("{}/repos/{REPO}/releases/latest", api_base.trim_end_matches('/'));
    let mut resp = http
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    match resp.status() {
        // A repo with no published release answers 404.
        StatusCode::NOT_FOUND => return Ok(None),
        s if !s.is_success() => return Err(format!("GitHub answered {s}")),
        _ => {}
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.without_url().to_string())? {
        if body.len() + chunk.len() > MAX_BODY {
            return Err("release response too large".into());
        }
        body.extend_from_slice(&chunk);
    }
    let release: Release = serde_json::from_slice(&body).map_err(|e| format!("unexpected release response: {e}"))?;
    Ok(newer(&release.tag_name, current))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const LATEST: &str = "/repos/anphonic/llama-swatch/releases/latest";

    #[test]
    fn versions_are_strict() {
        assert_eq!(parse_version("0.1.0"), Some((0, 1, 0)));
        assert_eq!(parse_version("10.20.300"), Some((10, 20, 300)));
        for bad in ["", "1", "1.2", "1.2.3.4", "1.2.3-beta", "1.2.3+b", "1..3", " 1.2.3", "1.2.x", "+1.2.3", "v1.2.3"] {
            assert_eq!(parse_version(bad), None, "{bad:?}");
        }
        assert_eq!(parse_tag("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_tag("1.2.3"), None, "tags carry the v");
        assert_eq!(parse_tag("V1.2.3"), None);
    }

    #[test]
    fn newer_compares_numerically() {
        assert_eq!(newer("v0.1.1", "0.1.0"), Some(UpdateInfo { tag: "v0.1.1".into() }));
        assert_eq!(newer("v0.10.0", "0.9.9").map(|u| u.tag), Some("v0.10.0".into()), "not a string compare");
        assert_eq!(newer("v1.0.0", "0.99.99").map(|u| u.tag), Some("v1.0.0".into()));
        assert_eq!(newer("v0.1.0", "0.1.0"), None, "same version");
        assert_eq!(newer("v0.0.9", "0.1.0"), None, "older");
        assert_eq!(newer("v9.9.9-rc1", "0.1.0"), None, "pre-release tags are ignored");
        assert_eq!(newer("v9.9.9", "dev"), None, "unparsable current version");
    }

    #[test]
    fn release_url_is_built_from_a_valid_tag_only() {
        assert_eq!(release_url("v0.2.0").as_deref(), Some("https://github.com/anphonic/llama-swatch/releases/tag/v0.2.0"));
        for bad in ["v0.2.0/../../evil", "https://evil.example", "v0.2", "", "v0.2.0?x=1"] {
            assert_eq!(release_url(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn current_crate_version_parses() {
        assert!(parse_version(env!("CARGO_PKG_VERSION")).is_some());
    }

    #[tokio::test]
    async fn reports_a_newer_release() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(LATEST))
            .and(header("Accept", "application/vnd.github+json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "tag_name": "v0.2.0",
                "html_url": "https://evil.example/ignored",
                "body": "notes"
            })))
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(check(&server.uri(), "0.1.0").await.unwrap(), Some(UpdateInfo { tag: "v0.2.0".into() }));
        let req = &server.received_requests().await.unwrap()[0];
        let ua = req.headers.get("user-agent").unwrap().to_str().unwrap();
        assert!(ua.starts_with("llama-swatch/"), "{ua}");
        assert!(req.headers.get("authorization").is_none(), "never authenticated");
    }

    #[tokio::test]
    async fn up_to_date_and_no_releases_are_none() {
        let server = MockServer::start().await;
        Mock::given(path(LATEST))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "tag_name": "v0.1.0" })))
            .mount(&server)
            .await;
        assert_eq!(check(&server.uri(), "0.1.0").await.unwrap(), None);

        let empty = MockServer::start().await;
        Mock::given(path(LATEST)).respond_with(ResponseTemplate::new(404)).mount(&empty).await;
        assert_eq!(check(&empty.uri(), "0.1.0").await.unwrap(), None);
    }

    #[tokio::test]
    async fn errors_redirects_and_junk_are_errors() {
        let server = MockServer::start().await;
        Mock::given(path(LATEST)).respond_with(ResponseTemplate::new(403)).mount(&server).await;
        assert!(check(&server.uri(), "0.1.0").await.unwrap_err().contains("403"));

        let redirect = MockServer::start().await;
        Mock::given(path(LATEST))
            .respond_with(ResponseTemplate::new(302).insert_header("Location", "https://evil.example/"))
            .mount(&redirect)
            .await;
        assert!(check(&redirect.uri(), "0.1.0").await.is_err(), "redirects are not followed");

        let junk = MockServer::start().await;
        Mock::given(path(LATEST)).respond_with(ResponseTemplate::new(200).set_body_string("<html>")).mount(&junk).await;
        assert!(check(&junk.uri(), "0.1.0").await.is_err());

        let huge = MockServer::start().await;
        let big = format!("{{\"tag_name\":\"v9.0.0\",\"body\":\"{}\"}}", "x".repeat(MAX_BODY));
        Mock::given(path(LATEST)).respond_with(ResponseTemplate::new(200).set_body_string(big)).mount(&huge).await;
        assert_eq!(check(&huge.uri(), "0.1.0").await.unwrap_err(), "release response too large");
    }
}
