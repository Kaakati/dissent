//! HTTP API: register, wake, unregister. Handlers log outcomes, never the wake token, the secret or
//! the endpoint address.

use crate::limit::Limiter;
use crate::net::check_unifiedpush_url;
use crate::push::{Delivery, Push};
use crate::store::{Endpoint, Registration, Removal, Store};
use crate::token::{decode_token, encode, hash_secret, hash_token, random_bytes, SECRET_BYTES, TOKEN_BYTES};
use crate::{Config, PushError};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode, header::AUTHORIZATION};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use data_encoding::BASE32_NOPAD;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tracing::{info, warn};

/// Per source IP, across all endpoints: a steady 2 per second, bursts of 30.
const IP_RATE_PER_SEC: f64 = 2.0;
const IP_BURST: f64 = 30.0;
/// Per wake token: one wake every 10 s, bursts of 5.
const TOKEN_RATE_PER_SEC: f64 = 0.1;
const TOKEN_BURST: f64 = 5.0;
/// Registration bodies are small. Anything bigger is refused.
const MAX_BODY_BYTES: usize = 4096;

pub struct AppState {
    pub cfg: Config,
    pub store: Store,
    pub push: Push,
    pub ip_limit: Limiter<IpAddr>,
    pub token_limit: Limiter<[u8; 32]>,
}

impl AppState {
    pub fn new(cfg: Config, push: Push) -> Result<Self, PushError> {
        let store = Store::open(cfg.snapshot.clone())?;
        Ok(Self {
            cfg,
            store,
            push,
            ip_limit: Limiter::new(IP_RATE_PER_SEC, IP_BURST),
            token_limit: Limiter::new(TOKEN_RATE_PER_SEC, TOKEN_BURST),
        })
    }
}

pub fn app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/register", post(register))
        .route("/register/{token}", delete(unregister))
        .route("/wake/{token}", post(wake))
        .route("/healthz", get(|| async { "ok" }))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}

#[derive(Deserialize)]
struct RegisterRequest {
    endpoint: Endpoint,
}

#[derive(Serialize)]
struct Registered {
    wake_token: String,
    secret: String,
}

fn text(status: StatusCode, message: impl Into<String>) -> Response {
    (status, message.into()).into_response()
}

async fn register(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(req): Json<RegisterRequest>,
) -> Response {
    if !state.ip_limit.allow(&peer.ip()) {
        return text(StatusCode::TOO_MANY_REQUESTS, "slow down");
    }
    match &req.endpoint {
        Endpoint::UnifiedPush { url } => {
            if let Err(e) = check_unifiedpush_url(url, &state.cfg).await {
                return text(StatusCode::BAD_REQUEST, e.to_string());
            }
        }
        Endpoint::Apns { device_token } => {
            if state.push.apns.is_none() {
                return text(StatusCode::BAD_REQUEST, "APNs is not enabled on this relay");
            }
            if !valid_device_token(device_token) {
                return text(StatusCode::BAD_REQUEST, "APNs device token must be hex");
            }
        }
    }
    let token = match random_bytes::<TOKEN_BYTES>() {
        Ok(t) => t,
        Err(e) => return server_error(e),
    };
    let secret = match random_bytes::<SECRET_BYTES>() {
        Ok(s) => s,
        Err(e) => return server_error(e),
    };
    state.store.insert(
        hash_token(&token),
        Registration {
            secret_hash: hash_secret(&secret),
            endpoint: req.endpoint,
        },
    );
    if let Err(e) = state.store.persist() {
        return server_error(e);
    }
    info!("registered a device");
    (
        StatusCode::CREATED,
        Json(Registered {
            wake_token: encode(&token),
            secret: BASE32_NOPAD.encode(&secret).to_ascii_lowercase(),
        }),
    )
        .into_response()
}

async fn unregister(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !state.ip_limit.allow(&peer.ip()) {
        return text(StatusCode::TOO_MANY_REQUESTS, "slow down");
    }
    let Some(token) = decode_token(&token) else {
        return text(StatusCode::NOT_FOUND, "no such registration");
    };
    let Some(secret) = bearer_secret(&headers) else {
        return text(StatusCode::UNAUTHORIZED, "send the secret as Authorization: Bearer <secret>");
    };
    match state
        .store
        .remove_with_secret(&hash_token(&token), &secret)
    {
        Removal::Removed => {
            if let Err(e) = state.store.persist() {
                return server_error(e);
            }
            info!("unregistered a device");
            StatusCode::NO_CONTENT.into_response()
        }
        Removal::NotFound => text(StatusCode::NOT_FOUND, "no such registration"),
        Removal::WrongSecret => text(StatusCode::FORBIDDEN, "wrong secret"),
    }
}

async fn wake(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(token): Path<String>,
) -> Response {
    if !state.ip_limit.allow(&peer.ip()) {
        return text(StatusCode::TOO_MANY_REQUESTS, "slow down");
    }
    let Some(token) = decode_token(&token) else {
        return text(StatusCode::NOT_FOUND, "no such registration");
    };
    let key = hash_token(&token);
    // Only known tokens get a bucket, so random guesses cannot fill memory.
    let Some(endpoint) = state.store.endpoint(&key) else {
        return text(StatusCode::NOT_FOUND, "no such registration");
    };
    if !state.token_limit.allow(&key) {
        return text(StatusCode::TOO_MANY_REQUESTS, "too many wake-ups for this device");
    }
    match state.push.wake(&state.cfg, &endpoint).await {
        Ok(Delivery::Sent) => StatusCode::ACCEPTED.into_response(),
        Ok(Delivery::Gone) => {
            state.store.remove(&key);
            if let Err(e) = state.store.persist() {
                warn!(error = %e, "could not save after dropping a gone endpoint");
            }
            info!("endpoint is gone; registration dropped");
            text(StatusCode::GONE, "the push endpoint no longer exists")
        }
        Err(e) => {
            warn!(error = %e, "wake failed");
            text(StatusCode::BAD_GATEWAY, "the push provider did not accept the wake")
        }
    }
}

fn server_error(e: PushError) -> Response {
    warn!(error = %e, "internal error");
    text(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
}

/// The secret from `Authorization: Bearer <secret>`, if well formed.
fn bearer_secret(headers: &HeaderMap) -> Option<[u8; SECRET_BYTES]> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let text = value.strip_prefix("Bearer ")?.trim();
    let raw = BASE32_NOPAD.decode(text.to_ascii_uppercase().as_bytes()).ok()?;
    <[u8; SECRET_BYTES]>::try_from(raw.as_slice()).ok()
}

/// APNs device tokens are hex. Apple's are 32 bytes today, so accept 32 or more.
fn valid_device_token(token: &str) -> bool {
    token.len() >= 64 && token.len().is_multiple_of(2) &&token.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_token_must_be_hex_of_apple_length() {
        assert!(valid_device_token(&"ab".repeat(32)));
        assert!(!valid_device_token("abc"));
        assert!(!valid_device_token(&"zz".repeat(32)));
    }

    #[test]
    fn bearer_secret_parses_the_header() {
        let secret = [9u8; SECRET_BYTES];
        let mut headers = HeaderMap::new();
        let value = format!("Bearer {}", encode(&secret));
        headers.insert(AUTHORIZATION, value.parse().unwrap());
        assert_eq!(bearer_secret(&headers), Some(secret));

        let mut bad = HeaderMap::new();
        bad.insert(AUTHORIZATION, "Basic xyz".parse().unwrap());
        assert_eq!(bearer_secret(&bad), None);
        assert_eq!(bearer_secret(&HeaderMap::new()), None);
    }
}
