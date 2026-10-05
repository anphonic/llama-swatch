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
    assert_eq!(err.to_string(), format!("llama-swap didn't start m: {body}"));
    assert_eq!(err, ClientError::LoadRefused(format!("llama-swap didn't start m: {body}")));
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
    assert_eq!(refused(client.load_model("nope").await), "llama-swap didn't start nope: model not found");
}

#[tokio::test]
async fn load_refusal_with_empty_body_names_the_status() {
    let server = MockServer::start().await;
    Mock::given(path("/upstream/m/")).respond_with(ResponseTemplate::new(418)).mount(&server).await;
    mount_running(&server, &[("m", "stopped")]).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    assert_eq!(refused(client.load_model("m").await), "llama-swap didn't start m: HTTP 418");
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
async fn load_refusal_when_running_is_unavailable() {
    let server = MockServer::start().await;
    Mock::given(path("/upstream/m/"))
        .respond_with(ResponseTemplate::new(404).set_body_string("no router for model"))
        .mount(&server)
        .await;
    Mock::given(path("/running")).respond_with(ResponseTemplate::new(500)).mount(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    assert_eq!(refused(client.load_model("m").await), "llama-swap didn't start m: no router for model");
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
    let reason = msg.strip_prefix("llama-swap didn't start m: ").unwrap_or_else(|| panic!("{msg}"));
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
async fn activity_rows_carry_content_type_for_the_history_view() {
    let server = MockServer::start().await;
    mount_healthy(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    let a = client.activity(100).await.unwrap();
    assert_eq!(a[0].resp_content_type, "text/event-stream");
    assert_eq!(a[1].resp_content_type, "application/json");
}
