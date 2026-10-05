mod common;

use std::time::Duration;

use common::*;
use llama_swap_monitor_lib::client::{test_connection, ClientError, LlamaSwapClient, TestResult};
use wiremock::matchers::{any, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn sends_bearer_key_and_parses_version() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .and(header("authorization", "Bearer s3cret"))
        .respond_with(json_body(VERSION))
        .mount(&server)
        .await;
    let client = LlamaSwapClient::new(&server.uri(), Some("s3cret".into())).unwrap();
    assert_eq!(client.version().await.unwrap().version, "v188");
}

#[tokio::test]
async fn parses_all_fixture_endpoints() {
    let server = MockServer::start().await;
    mount_healthy(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    let (body, _latency) = client.health().await.unwrap();
    assert_eq!(body, "OK");
    assert_eq!(client.running().await.unwrap().len(), 2);
    assert_eq!(client.models().await.unwrap().len(), 3);
    assert_eq!(client.stats().await.unwrap().total_requests, 42);
    assert_eq!(client.activity(100).await.unwrap().len(), 3);
}

#[tokio::test]
async fn status_codes_map_to_errors() {
    let server = MockServer::start().await;
    Mock::given(path("/running")).respond_with(ResponseTemplate::new(401)).mount(&server).await;
    Mock::given(path("/v1/models")).respond_with(ResponseTemplate::new(500)).mount(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    assert_eq!(client.running().await, Err(ClientError::Unauthorized));
    assert_eq!(client.stats().await, Err(ClientError::NotFound));
    assert_eq!(client.models().await, Err(ClientError::Http(500)));
}

#[tokio::test]
async fn html_body_is_a_decode_error() {
    let server = MockServer::start().await;
    Mock::given(path("/running"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>hi</html>"))
        .mount(&server)
        .await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    assert!(matches!(client.running().await, Err(ClientError::Decode(_))));
}

#[tokio::test]
async fn closed_port_is_unreachable() {
    let client = LlamaSwapClient::new(&closed_port_url(), None).unwrap();
    assert!(matches!(client.health().await, Err(ClientError::Unreachable(_))));
}

#[tokio::test]
async fn slow_server_times_out_as_unreachable() {
    let server = MockServer::start().await;
    Mock::given(path("/health"))
        .respond_with(ResponseTemplate::new(200).set_body_string("OK").set_delay(Duration::from_secs(5)))
        .mount(&server)
        .await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    assert_eq!(client.health().await, Err(ClientError::Unreachable("timed out".into())));
}

#[tokio::test]
async fn test_connection_ok() {
    let server = MockServer::start().await;
    mount_healthy(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    assert_eq!(test_connection(&client).await, TestResult::Ok { version: "v188".into() });
}

#[tokio::test]
async fn test_connection_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(path("/api/version")).respond_with(ResponseTemplate::new(401)).with_priority(1).mount(&server).await;
    mount_healthy(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), Some("wrong".into())).unwrap();
    assert_eq!(test_connection(&client).await, TestResult::Unauthorized);
}

#[tokio::test]
async fn test_connection_unreachable() {
    let client = LlamaSwapClient::new(&closed_port_url(), None).unwrap();
    assert!(matches!(test_connection(&client).await, TestResult::Unreachable { .. }));
}

#[tokio::test]
async fn test_connection_rejects_non_llama_swap() {
    // Some other web app that answers 200 with HTML everywhere.
    let html = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<!doctype html><title>Router</title>"))
        .mount(&html)
        .await;
    let client = LlamaSwapClient::new(&html.uri(), None).unwrap();
    assert!(matches!(test_connection(&client).await, TestResult::NotLlamaSwap { .. }));

    // A server with no /health route at all.
    let empty = MockServer::start().await;
    let client = LlamaSwapClient::new(&empty.uri(), None).unwrap();
    assert!(matches!(test_connection(&client).await, TestResult::NotLlamaSwap { .. }));
}

#[tokio::test]
async fn test_connection_old_llama_swap_without_version_endpoint() {
    let server = MockServer::start().await;
    Mock::given(path("/api/version")).respond_with(ResponseTemplate::new(404)).with_priority(1).mount(&server).await;
    mount_healthy(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    assert_eq!(test_connection(&client).await, TestResult::Ok { version: "unknown".into() });
}

#[tokio::test]
async fn test_connection_unauthorized_health() {
    let server = MockServer::start().await;
    Mock::given(path("/health")).respond_with(ResponseTemplate::new(401)).mount(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    assert_eq!(test_connection(&client).await, TestResult::Unauthorized);
}

#[tokio::test]
async fn load_model_gets_upstream_root_with_auth() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/upstream/qwen3-30b/"))
        .and(header("authorization", "Bearer s3cret"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>upstream root</html>"))
        .expect(1)
        .mount(&server)
        .await;
    let client = LlamaSwapClient::new(&server.uri(), Some("s3cret".into())).unwrap();
    client.load_model("qwen3-30b").await.unwrap();
}

#[tokio::test]
async fn unload_model_posts_with_auth() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/models/unload/qwen3-30b"))
        .and(header("authorization", "Bearer s3cret"))
        .respond_with(ResponseTemplate::new(200).set_body_string("OK"))
        .expect(1)
        .mount(&server)
        .await;
    let client = LlamaSwapClient::new(&server.uri(), Some("s3cret".into())).unwrap();
    client.unload_model("qwen3-30b").await.unwrap();
}

#[tokio::test]
async fn load_and_unload_omit_auth_without_a_key() {
    let server = MockServer::start().await;
    Mock::given(any()).respond_with(ResponseTemplate::new(200)).mount(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    client.load_model("m").await.unwrap();
    client.unload_model("m").await.unwrap();
    for r in server.received_requests().await.unwrap() {
        assert!(!r.headers.contains_key("authorization"), "{:?}", r.url);
    }
}

#[tokio::test]
async fn model_ids_are_encoded_into_a_single_path_segment() {
    let server = MockServer::start().await;
    Mock::given(any()).respond_with(ResponseTemplate::new(200)).mount(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    let ids = ["a/b", "../x", "a b", "a#b", "a?b=1", "..%2f", "100%", "a\\b", "ünï", "x..y"];
    for id in ids {
        client.load_model(id).await.unwrap();
        client.unload_model(id).await.unwrap();
    }
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs.len(), ids.len() * 2);
    for r in &reqs {
        assert_eq!(r.url.query(), None, "{}", r.url);
        assert_eq!(r.url.fragment(), None, "{}", r.url);
        let segs: Vec<&str> = r.url.path().trim_start_matches('/').split('/').collect();
        let upstream = r.method.as_str() == "GET";
        // upstream/{id}/ -> ["upstream", id, ""]; api/models/unload/{id} -> 4 segments
        assert_eq!(segs.len(), if upstream { 3 } else { 4 }, "{}", r.url);
        assert!(!segs.contains(&"..") && !segs.contains(&"."), "{}", r.url);
    }
    // spot-check exact encodings
    let first = reqs[0].url.path().to_string();
    assert_eq!(first, "/upstream/a%2Fb/");
    assert_eq!(reqs[1].url.path(), "/api/models/unload/a%2Fb");
    assert_eq!(reqs[2].url.path(), "/upstream/..%2Fx/");
    assert_eq!(reqs[4].url.path(), "/upstream/a%20b/");
    assert_eq!(reqs[6].url.path(), "/upstream/a%23b/");
    assert_eq!(reqs[8].url.path(), "/upstream/a%3Fb%3D1/");
}

#[tokio::test]
async fn dot_segment_ids_are_rejected_without_a_request() {
    let server = MockServer::start().await;
    Mock::given(any()).respond_with(ResponseTemplate::new(200)).expect(0).mount(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    for id in ["..", ".", "", "  "] {
        assert!(client.load_model(id).await.is_err(), "{id:?}");
        assert!(client.unload_model(id).await.is_err(), "{id:?}");
    }
}

#[tokio::test]
async fn load_and_unload_redirects_are_reported_not_followed() {
    let server = MockServer::start().await;
    let target = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(307).insert_header("Location", format!("{}/upstream/m/", target.uri())))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(any()).respond_with(ResponseTemplate::new(200)).expect(0).mount(&target).await;
    let client = LlamaSwapClient::new(&server.uri(), Some("k".into())).unwrap();
    for res in [client.load_model("m").await, client.unload_model("m").await] {
        let Err(ClientError::Redirect(msg)) = res else { panic!("expected Redirect, got {res:?}") };
        assert!(msg.contains("redirected"), "{msg}");
        assert!(!msg.contains("/upstream/m"), "raw Location leaked: {msg}");
    }
}

#[tokio::test]
async fn load_and_unload_map_error_statuses() {
    let server = MockServer::start().await;
    Mock::given(path("/upstream/gone/")).respond_with(ResponseTemplate::new(502)).mount(&server).await;
    Mock::given(path("/api/models/unload/bad")).respond_with(ResponseTemplate::new(500)).mount(&server).await;
    Mock::given(path("/upstream/locked/")).respond_with(ResponseTemplate::new(401)).mount(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    assert_eq!(client.load_model("gone").await, Err(ClientError::Http(502)));
    assert_eq!(client.unload_model("bad").await, Err(ClientError::Http(500)));
    assert_eq!(client.load_model("locked").await, Err(ClientError::Unauthorized));
    assert!(matches!(
        LlamaSwapClient::new(&closed_port_url(), None).unwrap().load_model("m").await,
        Err(ClientError::Unreachable(_))
    ));
}

/// Mounts `/running` listing `models` as `(id, state)` pairs.
async fn mount_running(server: &MockServer, models: &[(&str, &str)]) {
    let running: Vec<_> = models.iter().map(|(m, s)| serde_json::json!({"model": m, "state": s})).collect();
    Mock::given(method("GET"))
        .and(path("/running"))
        .respond_with(json_body(&serde_json::json!({ "running": running }).to_string()))
        .mount(server)
        .await;
}

fn refused(res: Result<(), ClientError>) -> String {
    match res {
        Err(ClientError::LoadRefused(msg)) => msg,
        other => panic!("expected LoadRefused, got {other:?}"),
    }
}

#[tokio::test]
async fn load_refused_by_ignore_paths_reports_llama_swaps_reason() {
    let server = MockServer::start().await;
    let body = "model m is not loaded; path matches upstream.ignorePaths";
    Mock::given(path("/upstream/m/")).respond_with(ResponseTemplate::new(409).set_body_string(body)).mount(&server).await;
    mount_running(&server, &[("other", "ready")]).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    let err = client.load_model("m").await.unwrap_err();
    assert_eq!(err, ClientError::LoadRefused(body.into()));
    assert_eq!(err.to_string(), format!("llama-swap refused it: {body}"));
}

#[tokio::test]
async fn load_of_unknown_model_is_refused() {
    let server = MockServer::start().await;
    Mock::given(path("/upstream/nope/"))
        .respond_with(ResponseTemplate::new(404).set_body_string("model not found\n"))
        .mount(&server)
        .await;
    mount_running(&server, &[]).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    assert_eq!(refused(client.load_model("nope").await), "model not found");
}

#[tokio::test]
async fn load_refusal_with_empty_body_names_the_status() {
    let server = MockServer::start().await;
    Mock::given(path("/upstream/m/")).respond_with(ResponseTemplate::new(418)).mount(&server).await;
    mount_running(&server, &[("m", "stopped")]).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    assert_eq!(refused(client.load_model("m").await), "HTTP 418");
}

#[tokio::test]
async fn load_4xx_from_a_started_upstream_is_ok() {
    // llama-server with no root page answers 404 once llama-swap has started it.
    let server = MockServer::start().await;
    Mock::given(path("/upstream/qwen3-30b/")).respond_with(ResponseTemplate::new(404)).mount(&server).await;
    Mock::given(path("/upstream/embed/")).respond_with(ResponseTemplate::new(404)).mount(&server).await;
    Mock::given(method("GET")).and(path("/running")).respond_with(json_body(RUNNING)).expect(2).mount(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    assert_eq!(client.load_model("qwen3-30b").await, Ok(())); // ready
    assert_eq!(client.load_model("embed").await, Ok(())); // starting
}

#[tokio::test]
async fn load_2xx_is_ok_without_checking_running() {
    let server = MockServer::start().await;
    Mock::given(path("/upstream/m/")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
    Mock::given(path("/running")).respond_with(json_body(RUNNING)).expect(0).mount(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    assert_eq!(client.load_model("m").await, Ok(()));
}

#[tokio::test]
async fn load_5xx_is_an_error_without_checking_running() {
    let server = MockServer::start().await;
    Mock::given(path("/upstream/m/")).respond_with(ResponseTemplate::new(500).set_body_string("boom")).mount(&server).await;
    Mock::given(path("/running")).respond_with(json_body(RUNNING)).expect(0).mount(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    assert_eq!(client.load_model("m").await, Err(ClientError::Http(500)));
    // unload keeps strict status handling
    Mock::given(path("/api/models/unload/x")).respond_with(ResponseTemplate::new(404)).mount(&server).await;
    assert_eq!(client.unload_model("x").await, Err(ClientError::NotFound));
}

#[tokio::test]
async fn load_refused_when_model_is_stopping_or_listed_under_another_id() {
    let server = MockServer::start().await;
    Mock::given(path("/upstream/m/")).respond_with(ResponseTemplate::new(409).set_body_string("no")).mount(&server).await;
    Mock::given(path("/upstream/n/")).respond_with(ResponseTemplate::new(404).set_body_string("no")).mount(&server).await;
    mount_running(&server, &[("m", "stopping"), ("n-other", "ready")]).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    assert_eq!(refused(client.load_model("m").await), "no");
    assert_eq!(refused(client.load_model("n").await), "no");
}

#[tokio::test]
async fn load_403_is_unauthorized_without_checking_running() {
    let server = MockServer::start().await;
    Mock::given(path("/upstream/m/")).respond_with(ResponseTemplate::new(403)).mount(&server).await;
    Mock::given(path("/running")).respond_with(json_body(RUNNING)).expect(0).mount(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    assert_eq!(client.load_model("m").await, Err(ClientError::Unauthorized));
}

fn unconfirmed(res: Result<(), ClientError>) -> String {
    match res {
        Err(e @ ClientError::LoadUnconfirmed(_)) => e.to_string(),
        other => panic!("expected LoadUnconfirmed, got {other:?}"),
    }
}

#[tokio::test]
async fn load_is_unconfirmed_when_running_fails() {
    let server = MockServer::start().await;
    Mock::given(path("/upstream/m/"))
        .respond_with(ResponseTemplate::new(404).set_body_string("no router for model"))
        .mount(&server)
        .await;
    Mock::given(path("/upstream/e/")).respond_with(ResponseTemplate::new(404)).mount(&server).await;
    Mock::given(path("/running")).respond_with(ResponseTemplate::new(500)).mount(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    assert_eq!(
        unconfirmed(client.load_model("m").await),
        "llama-swap answered HTTP 404 and the model list could not be checked: no router for model"
    );
    assert_eq!(
        unconfirmed(client.load_model("e").await),
        "llama-swap answered HTTP 404 and the model list could not be checked"
    );
}

#[tokio::test]
async fn load_is_unconfirmed_when_running_times_out() {
    // /running uses the 3 s request timeout; the client gives up long before this answer.
    let server = MockServer::start().await;
    Mock::given(path("/upstream/m/")).respond_with(ResponseTemplate::new(404).set_body_string("nope")).mount(&server).await;
    Mock::given(path("/running"))
        .respond_with(json_body(RUNNING).set_delay(Duration::from_secs(10)))
        .mount(&server)
        .await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    let msg = unconfirmed(client.load_model("m").await);
    assert!(msg.ends_with("could not be checked: nope"), "{msg}");
}

#[tokio::test]
async fn load_refusal_body_is_capped_and_sanitized() {
    let server = MockServer::start().await;
    let mut body = String::from("bad\u{1b}[31m\r\nthing\u{0}\u{202e}here ");
    body.push_str(&"x".repeat(1_000_000));
    Mock::given(path("/upstream/m/")).respond_with(ResponseTemplate::new(409).set_body_string(body)).mount(&server).await;
    mount_running(&server, &[]).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    let msg = refused(client.load_model("m").await);
    let reason = msg.as_str();
    assert!(reason.starts_with("bad [31m thing here xxx"), "{reason}");
    assert!(reason.chars().count() <= 200, "{}", reason.chars().count());
    assert!(!msg.chars().any(|c| c.is_control() || c == '\u{202e}'), "{msg:?}");
}

#[tokio::test]
async fn load_refusal_never_echoes_the_key_or_credentials() {
    let server = MockServer::start().await;
    let echo = "denied Bearer s3cret-key from http://user:pw@example.com/x";
    Mock::given(path("/upstream/m/")).respond_with(ResponseTemplate::new(409).set_body_string(echo)).mount(&server).await;
    mount_running(&server, &[]).await;
    let client = LlamaSwapClient::new(&server.uri(), Some("s3cret-key".into())).unwrap();
    let msg = refused(client.load_model("m").await);
    assert!(!msg.contains("s3cret-key"), "{msg}");
    assert!(!msg.contains("user:pw"), "{msg}");
    assert!(msg.contains("http://example.com/x"), "{msg}");
}

#[tokio::test]
async fn load_refusal_drops_url_queries_and_fragments() {
    let server = MockServer::start().await;
    let body = "see http://example.com/a?token=abc&x=1#frag and https://example.com/b#access_token=t done";
    Mock::given(path("/upstream/m/")).respond_with(ResponseTemplate::new(409).set_body_string(body)).mount(&server).await;
    mount_running(&server, &[]).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    assert_eq!(
        refused(client.load_model("m").await),
        "see http://example.com/a and https://example.com/b done"
    );
}

#[tokio::test]
async fn load_refusal_never_leaks_a_key_cut_at_the_body_cap() {
    let key = "s3cret-key-0123456789";
    // Whitespace collapses, so text near the 4096-byte cap lands in the shown reason.
    for start in [4090, 4096 - key.len() + 1, 4095, 4100] {
        let server = MockServer::start().await;
        let body = format!("{}{key} tail", " ".repeat(start));
        Mock::given(path("/upstream/m/")).respond_with(ResponseTemplate::new(409).set_body_string(body)).mount(&server).await;
        mount_running(&server, &[]).await;
        let client = LlamaSwapClient::new(&server.uri(), Some(key.into())).unwrap();
        let msg = refused(client.load_model("m").await);
        assert!(!msg.contains('s'), "start {start}: {msg}"); // no part of the key, however short
    }
    // A bordered key (starts and ends with "k3y") that the body ends with exactly.
    let key = "k3y-abc-k3y";
    for start in [4090, 4092, 4095, 4096, 4100] {
        let server = MockServer::start().await;
        let body = format!("{}{key}", " ".repeat(start));
        Mock::given(path("/upstream/m/")).respond_with(ResponseTemplate::new(409).set_body_string(body)).mount(&server).await;
        mount_running(&server, &[]).await;
        let client = LlamaSwapClient::new(&server.uri(), Some(key.into())).unwrap();
        let msg = refused(client.load_model("m").await);
        assert!(!msg.contains('k') && !msg.contains("3y"), "start {start}: {msg}");
    }
    // A key longer than the read-ahead past the cap is cut, and its read part still never shows.
    let key: String = (0..400).map(|i| b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"[i * 7 % 36] as char).collect();
    let server = MockServer::start().await;
    let body = format!("{}{key} tail", " ".repeat(4000));
    Mock::given(path("/upstream/m/")).respond_with(ResponseTemplate::new(409).set_body_string(body)).mount(&server).await;
    mount_running(&server, &[]).await;
    let client = LlamaSwapClient::new(&server.uri(), Some(key.clone())).unwrap();
    let msg = refused(client.load_model("m").await);
    assert_eq!(msg, "<redacted>");
}

#[tokio::test]
async fn activity_rows_carry_content_type_for_the_history_view() {
    let server = MockServer::start().await;
    mount_healthy(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    let a = client.activity(100).await.unwrap();
    assert_eq!(a[0].resp_content_type, "text/event-stream");
    assert_eq!(a[1].resp_content_type, "application/json");
}
