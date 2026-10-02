mod common;

use common::*;
use llama_swap_monitor_lib::client::LlamaSwapClient;
use llama_swap_monitor_lib::poller::{poll_once, Feature, PollOutcome};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn unwrap_ok(o: PollOutcome) -> llama_swap_monitor_lib::poller::PollData {
    match o {
        PollOutcome::Ok(d) => d,
        other => panic!("expected Ok, got {other:?}"),
    }
}

#[tokio::test]
async fn full_poll_against_healthy_server() {
    let server = MockServer::start().await;
    mount_healthy(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    let d = unwrap_ok(poll_once(&client, true, true).await);
    assert_eq!(d.running.len(), 2);
    assert_eq!(d.models.unwrap().len(), 3);
    assert_eq!(d.version.unwrap().version, "v188");
    assert!(matches!(d.stats, Feature::Available(ref s) if s.total_requests == 42));
    assert!(matches!(d.activity, Feature::Available(ref a) if a.len() == 3));
}

#[tokio::test]
async fn skips_models_and_version_when_not_wanted() {
    let server = MockServer::start().await;
    mount_healthy(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    let d = unwrap_ok(poll_once(&client, false, false).await);
    assert!(d.models.is_none());
    assert!(d.version.is_none());
    let requests = server.received_requests().await.unwrap();
    assert!(!requests.iter().any(|r| r.url.path() == "/v1/models" || r.url.path() == "/api/version"));
}

#[tokio::test]
async fn missing_metrics_endpoints_are_unavailable() {
    let server = MockServer::start().await;
    Mock::given(path("/health")).respond_with(ResponseTemplate::new(200).set_body_string("OK")).mount(&server).await;
    Mock::given(path("/running")).respond_with(json_body(RUNNING)).mount(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    let d = unwrap_ok(poll_once(&client, false, false).await);
    assert_eq!(d.stats, Feature::Unavailable);
    assert_eq!(d.activity, Feature::Unavailable);
}

#[tokio::test]
async fn transient_stats_error_is_failed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/metrics/stats"))
        .respond_with(ResponseTemplate::new(500))
        .with_priority(1)
        .mount(&server)
        .await;
    mount_healthy(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    let d = unwrap_ok(poll_once(&client, false, false).await);
    assert_eq!(d.stats, Feature::Failed);
}

#[tokio::test]
async fn unauthorized_running_is_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(path("/running")).respond_with(ResponseTemplate::new(401)).with_priority(1).mount(&server).await;
    mount_healthy(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    assert_eq!(poll_once(&client, true, true).await, PollOutcome::Unauthorized);
}

#[tokio::test]
async fn down_server_is_unreachable() {
    let client = LlamaSwapClient::new(&closed_port_url(), None).unwrap();
    assert!(matches!(poll_once(&client, true, true).await, PollOutcome::Unreachable(_)));
}
