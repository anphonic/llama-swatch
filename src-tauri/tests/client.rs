mod common;

use std::time::Duration;

use common::*;
use llama_swap_monitor_lib::client::{test_connection, ClientError, LlamaSwapClient, TestResult};
use wiremock::matchers::{header, method, path};
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
