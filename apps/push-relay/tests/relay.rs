//! End-to-end tests of the wake relay over HTTP, on loopback. A fake UnifiedPush endpoint stands in
//! for a distributor. Nothing here reaches the public internet.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use dissent_push_relay::{Config, app, build_state};
use reqwest::Client;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<(HeaderMap, usize)>>>);

impl Seen {
    fn posts(&self) -> Vec<(HeaderMap, usize)> {
        self.0.lock().unwrap().clone()
    }
}

async fn fake_endpoint(State(seen): State<Seen>, headers: HeaderMap, body: Bytes) -> StatusCode {
    seen.0.lock().unwrap().push((headers, body.len()));
    StatusCode::CREATED
}

/// A UnifiedPush stand-in on loopback. Returns its URL and a record of what it received.
async fn fake_unifiedpush() -> (String, Seen) {
    let seen = Seen::default();
    let router = axum::Router::new()
        .route("/up", post(fake_endpoint))
        .with_state(seen.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{addr}/up"), seen)
}

/// Starts a relay on loopback. Returns its base URL.
async fn relay(dev: bool, snapshot: Option<PathBuf>) -> String {
    let cfg = Config {
        dev,
        ups_allow_hosts: Vec::new(),
        snapshot,
    };
    let state = build_state(cfg, None).expect("state");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app(state).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    format!("http://{addr}")
}

struct Registered {
    token: String,
    secret: String,
}

async fn register(client: &Client, base: &str, endpoint: Value) -> reqwest::Response {
    client
        .post(format!("{base}/register"))
        .json(&json!({ "endpoint": endpoint }))
        .send()
        .await
        .unwrap()
}

async fn register_ok(client: &Client, base: &str, url: &str) -> Registered {
    let resp = register(client, base, json!({"kind": "unifiedpush", "url": url})).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body: Value = resp.json().await.unwrap();
    Registered {
        token: body["wake_token"].as_str().unwrap().to_string(),
        secret: body["secret"].as_str().unwrap().to_string(),
    }
}

async fn wake(client: &Client, base: &str, token: &str) -> StatusCode {
    client
        .post(format!("{base}/wake/{token}"))
        .send()
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn wake_sends_one_empty_high_urgency_post() {
    let (url, seen) = fake_unifiedpush().await;
    let base = relay(true, None).await;
    let client = Client::new();
    let reg = register_ok(&client, &base, &url).await;

    assert_eq!(wake(&client, &base, &reg.token).await, StatusCode::ACCEPTED);

    let posts = seen.posts();
    assert_eq!(posts.len(), 1, "exactly one POST");
    let (headers, body_len) = &posts[0];
    assert_eq!(*body_len, 0, "the wake carries no body");
    assert_eq!(headers.get("urgency").unwrap(), "high");
}

#[tokio::test]
async fn wakes_are_rate_limited_per_token() {
    let (url, seen) = fake_unifiedpush().await;
    let base = relay(true, None).await;
    let client = Client::new();
    let reg = register_ok(&client, &base, &url).await;

    let mut statuses = Vec::new();
    for _ in 0..6 {
        statuses.push(wake(&client, &base, &reg.token).await);
    }
    let accepted = statuses.iter().filter(|s| **s == StatusCode::ACCEPTED).count();
    assert_eq!(accepted, 5, "a burst of five, then refused: {statuses:?}");
    assert_eq!(statuses[5], StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(seen.posts().len(), 5, "refused wakes never reach the endpoint");
}

#[tokio::test]
async fn unregister_needs_the_secret() {
    let (url, _seen) = fake_unifiedpush().await;
    let base = relay(true, None).await;
    let client = Client::new();
    let reg = register_ok(&client, &base, &url).await;
    let other = register_ok(&client, &base, &url).await;

    let delete = |token: String, auth: Option<String>| {
        let client = client.clone();
        let base = base.clone();
        async move {
            let mut req = client.delete(format!("{base}/register/{token}"));
            if let Some(a) = auth {
                req = req.header("authorization", a);
            }
            req.send().await.unwrap().status()
        }
    };

    assert_eq!(
        delete(reg.token.clone(), None).await,
        StatusCode::UNAUTHORIZED,
        "no secret"
    );
    assert_eq!(
        delete(reg.token.clone(), Some(format!("Bearer {}", other.secret))).await,
        StatusCode::FORBIDDEN,
        "another registration's secret"
    );
    assert_eq!(
        delete(reg.token.clone(), Some(format!("Bearer {}", reg.secret))).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        wake(&client, &base, &reg.token).await,
        StatusCode::NOT_FOUND,
        "unregistered tokens no longer wake"
    );
    assert_eq!(
        wake(&client, &base, &other.token).await,
        StatusCode::ACCEPTED,
        "the other registration is untouched"
    );
}

#[tokio::test]
async fn private_endpoints_are_refused_without_dev() {
    let base = relay(false, None).await;
    let client = Client::new();
    for url in [
        "https://10.0.0.5/up",
        "https://169.254.169.254/latest/meta-data",
        "https://192.168.1.10/up",
        "http://8.8.8.8/up",
    ] {
        let resp = register(&client, &base, json!({"kind": "unifiedpush", "url": url})).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{url} must be refused");
    }
}

#[tokio::test]
async fn apns_is_refused_when_not_configured() {
    let base = relay(true, None).await;
    let client = Client::new();
    let resp = register(
        &client,
        &base,
        json!({"kind": "apns", "device_token": "ab".repeat(32)}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn unknown_and_malformed_tokens_are_not_found() {
    let base = relay(true, None).await;
    let client = Client::new();
    assert_eq!(wake(&client, &base, "aaaaaaaaaaaaaaaaaaaaaaaaaa").await, StatusCode::NOT_FOUND);
    assert_eq!(wake(&client, &base, "not-a-token").await, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn snapshot_hides_tokens_and_survives_a_restart() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!(
        "dissent-push-relay-it-{}-{nanos}.json",
        std::process::id()
    ));
    let (url, seen) = fake_unifiedpush().await;
    let client = Client::new();

    let base = relay(true, Some(path.clone())).await;
    let reg = register_ok(&client, &base, &url).await;

    let dump = std::fs::read_to_string(&path).expect("snapshot written");
    assert!(!dump.contains(&reg.token), "wake token must not appear in the snapshot");
    assert!(!dump.contains(&reg.secret), "delete secret must not appear in the snapshot");

    // A new relay started from the snapshot still wakes the device.
    let restarted = relay(true, Some(path.clone())).await;
    assert_eq!(wake(&client, &restarted, &reg.token).await, StatusCode::ACCEPTED);
    assert_eq!(seen.posts().len(), 1);
    std::fs::remove_file(&path).unwrap();
}
