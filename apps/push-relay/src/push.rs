//! Sends the empty wake-up. UnifiedPush is a plain POST with no body. APNs needs HTTP/2 with a
//! provider token. Neither request carries sender, recipient or message data.

use crate::apns::{ApnsSettings, ApnsSigner, build_request};
use crate::net::check_unifiedpush_url;
use crate::store::Endpoint;
use crate::{Config, PushError};
use reqwest::redirect::Policy;
use reqwest::{Client, StatusCode};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);

/// What the provider said about the wake.
#[derive(Debug, PartialEq, Eq)]
pub enum Delivery {
    Sent,
    /// The endpoint no longer exists. The registration should be dropped.
    Gone,
}

/// APNs state, present only when the operator configured APNs.
pub struct ApnsSender {
    pub signer: ApnsSigner,
    pub settings: ApnsSettings,
}

pub struct Push {
    pub http: Client,
    pub apns: Option<ApnsSender>,
}

impl Push {
    pub fn new(apns: Option<ApnsSettings>) -> Result<Self, PushError> {
        let http = Client::builder()
            .timeout(TIMEOUT)
            .redirect(Policy::none())
            .build()
            .map_err(|e| PushError::Transport(e.to_string()))?;
        let apns = match apns {
            Some(settings) => Some(ApnsSender {
                signer: ApnsSigner::new(&settings)?,
                settings,
            }),
            None => None,
        };
        Ok(Self { http, apns })
    }

    /// Sends one empty wake to `endpoint`.
    pub async fn wake(&self, cfg: &Config, endpoint: &Endpoint) -> Result<Delivery, PushError> {
        match endpoint {
            Endpoint::UnifiedPush { url } => self.wake_unifiedpush(cfg, url).await,
            Endpoint::Apns { device_token } => self.wake_apns(device_token).await,
        }
    }

    async fn wake_unifiedpush(&self, cfg: &Config, url: &str) -> Result<Delivery, PushError> {
        // Check the address again at send time, and send to the addresses that were checked.
        let target = check_unifiedpush_url(url, cfg).await?;
        let client = match &target.pin {
            Some((host, addrs)) => Client::builder()
                .timeout(TIMEOUT)
                .redirect(Policy::none())
                .resolve_to_addrs(host, addrs)
                .build(),
            None => Client::builder()
                .timeout(TIMEOUT)
                .redirect(Policy::none())
                .build(),
        }
        .map_err(|e| PushError::Transport(e.to_string()))?;
        let response = client
            .post(target.url.clone())
            .header("TTL", "300")
            .header("Urgency", "high")
            .header("Content-Length", "0")
            .send()
            .await
            .map_err(|e| PushError::Transport(e.to_string()))?;
        classify(response.status(), &[StatusCode::NOT_FOUND, StatusCode::GONE])
    }

    async fn wake_apns(&self, device_token: &str) -> Result<Delivery, PushError> {
        let apns = self.apns.as_ref().ok_or(PushError::ApnsDisabled)?;
        let jwt = apns.signer.current_jwt()?;
        let req = build_request(
            apns.settings.sandbox,
            device_token,
            &apns.settings.topic,
            &jwt,
        );
        let mut builder = self.http.post(&req.url);
        for (name, value) in &req.headers {
            builder = builder.header(*name, value);
        }
        let response = builder
            .body(req.body)
            .send()
            .await
            .map_err(|e| PushError::Transport(e.to_string()))?;
        classify(response.status(), &[StatusCode::GONE])
    }
}

fn classify(status: StatusCode, gone: &[StatusCode]) -> Result<Delivery, PushError> {
    if status.is_success() {
        Ok(Delivery::Sent)
    } else if gone.contains(&status) {
        Ok(Delivery::Gone)
    } else {
        Err(PushError::Upstream(status.as_u16()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_map_to_delivery() {
        assert_eq!(classify(StatusCode::CREATED, &[]).unwrap(), Delivery::Sent);
        assert_eq!(classify(StatusCode::OK, &[]).unwrap(), Delivery::Sent);
        assert_eq!(
            classify(StatusCode::GONE, &[StatusCode::NOT_FOUND, StatusCode::GONE]).unwrap(),
            Delivery::Gone
        );
        assert!(matches!(
            classify(StatusCode::INTERNAL_SERVER_ERROR, &[StatusCode::GONE]),
            Err(PushError::Upstream(500))
        ));
    }
}
