//! APNs request builder and provider-token (JWT) signer. Not exercised against Apple: there are no
//! credentials here, so the tests check the bytes this module produces.

use crate::PushError;
use data_encoding::{BASE64, BASE64URL_NOPAD};
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const PRODUCTION_BASE: &str = "https://api.push.apple.com";
pub const SANDBOX_BASE: &str = "https://api.sandbox.push.apple.com";

/// Apple accepts a provider token for 60 minutes and asks for a new one no more often than every
/// 20. Re-sign after 40.
const TOKEN_REUSE_FOR: Duration = Duration::from_secs(40 * 60);

/// What the operator gives the relay to talk to APNs.
#[derive(Clone, Debug)]
pub struct ApnsSettings {
    /// PKCS#8 PEM text of the `.p8` key from the developer portal.
    pub key_pem: String,
    /// 10-character key id.
    pub key_id: String,
    /// 10-character team id, the JWT issuer.
    pub team_id: String,
    /// The app's bundle id, sent as `apns-topic`.
    pub topic: String,
    /// Use the sandbox host.
    pub sandbox: bool,
}

/// Signs provider tokens and caches the current one.
pub struct ApnsSigner {
    key: EcdsaKeyPair,
    rng: SystemRandom,
    key_id: String,
    team_id: String,
    cached: Mutex<Option<(SystemTime, String)>>,
}

impl ApnsSigner {
    pub fn new(settings: &ApnsSettings) -> Result<Self, PushError> {
        let der = pkcs8_from_pem(&settings.key_pem)?;
        let rng = SystemRandom::new();
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &der, &rng)
            .map_err(|_| PushError::Key("the APNs key is not a P-256 PKCS#8 key".into()))?;
        Ok(Self {
            key,
            rng,
            key_id: settings.key_id.clone(),
            team_id: settings.team_id.clone(),
            cached: Mutex::new(None),
        })
    }

    /// A provider token valid now, re-signed when the cached one is old.
    pub fn current_jwt(&self) -> Result<String, PushError> {
        self.current_jwt_at(SystemTime::now())
    }

    pub fn current_jwt_at(&self, now: SystemTime) -> Result<String, PushError> {
        let mut cached = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((issued, jwt)) = cached.as_ref() {
            if now.duration_since(*issued).unwrap_or(Duration::ZERO) < TOKEN_REUSE_FOR {
                return Ok(jwt.clone());
            }
        }
        let iat = now
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let jwt = self.sign(iat)?;
        *cached = Some((now, jwt.clone()));
        Ok(jwt)
    }

    /// Builds `header.payload.signature` with ES256. `iat` is seconds since the epoch.
    pub fn sign(&self, iat: u64) -> Result<String, PushError> {
        let header = format!(r#"{{"alg":"ES256","kid":"{}"}}"#, self.key_id);
        let claims = format!(r#"{{"iss":"{}","iat":{iat}}}"#, self.team_id);
        let signing_input = format!(
            "{}.{}",
            BASE64URL_NOPAD.encode(header.as_bytes()),
            BASE64URL_NOPAD.encode(claims.as_bytes())
        );
        let sig = self
            .key
            .sign(&self.rng, signing_input.as_bytes())
            .map_err(|_| PushError::Key("signing failed".into()))?;
        Ok(format!(
            "{signing_input}.{}",
            BASE64URL_NOPAD.encode(sig.as_ref())
        ))
    }

    /// The public key, uncompressed. Lets tests verify tokens.
    pub fn public_key(&self) -> &[u8] {
        self.key.public_key().as_ref()
    }
}

/// The DER bytes inside a PEM `PRIVATE KEY` block.
pub fn pkcs8_from_pem(pem: &str) -> Result<Vec<u8>, PushError> {
    let body: String = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .map(str::trim)
        .collect();
    if body.is_empty() {
        return Err(PushError::Key("no PEM body in the APNs key".into()));
    }
    BASE64
        .decode(body.as_bytes())
        .map_err(|e| PushError::Key(format!("bad base64 in the APNs key: {e}")))
}

/// An HTTP/2 request to APNs, not yet sent.
#[derive(Debug, PartialEq, Eq)]
pub struct ApnsRequest {
    pub url: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

/// The background, content-available push. It carries no user content. `apns-priority` is 5
/// because APNs requires that for background pushes. Priority 10 is for alerts.
pub fn build_request(sandbox: bool, device_token: &str, topic: &str, jwt: &str) -> ApnsRequest {
    let base = if sandbox { SANDBOX_BASE } else { PRODUCTION_BASE };
    ApnsRequest {
        url: format!("{base}/3/device/{device_token}"),
        headers: vec![
            ("authorization", format!("bearer {jwt}")),
            ("apns-topic", topic.to_string()),
            ("apns-push-type", "background".to_string()),
            ("apns-priority", "5".to_string()),
            ("apns-expiration", "0".to_string()),
        ],
        body: br#"{"aps":{"content-available":1}}"#.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};

    /// A throwaway P-256 key in PKCS#8 PEM form, generated at test time.
    fn test_key_pem() -> String {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let b64 = BASE64.encode(pkcs8.as_ref());
        let lines: Vec<String> = b64
            .as_bytes()
            .chunks(64)
            .map(|c| String::from_utf8(c.to_vec()).unwrap())
            .collect();
        format!("-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n", lines.join("\n"))
    }

    fn signer() -> ApnsSigner {
        ApnsSigner::new(&ApnsSettings {
            key_pem: test_key_pem(),
            key_id: "ABC1234567".into(),
            team_id: "TEAM123456".into(),
            topic: "org.example.dissent".into(),
            sandbox: true,
        })
        .unwrap()
    }

    #[test]
    fn jwt_has_three_parts_and_verifies_against_the_key() {
        let s = signer();
        let jwt = s.sign(1_700_000_000).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);

        let header = String::from_utf8(BASE64URL_NOPAD.decode(parts[0].as_bytes()).unwrap()).unwrap();
        assert_eq!(header, r#"{"alg":"ES256","kid":"ABC1234567"}"#);
        let claims = String::from_utf8(BASE64URL_NOPAD.decode(parts[1].as_bytes()).unwrap()).unwrap();
        assert_eq!(claims, r#"{"iss":"TEAM123456","iat":1700000000}"#);

        let sig = BASE64URL_NOPAD.decode(parts[2].as_bytes()).unwrap();
        assert_eq!(sig.len(), 64, "ES256 signatures are fixed-size r||s");
        let key = UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, s.public_key());
        key.verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig)
            .expect("signature verifies");
    }

    #[test]
    fn jwt_is_reused_then_refreshed() {
        let s = signer();
        let t0 = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let first = s.current_jwt_at(t0).unwrap();
        let again = s.current_jwt_at(t0 + Duration::from_secs(60)).unwrap();
        assert_eq!(first, again);
        let later = s.current_jwt_at(t0 + Duration::from_secs(41 * 60)).unwrap();
        assert_ne!(first, later);
    }

    #[test]
    fn request_matches_the_background_push_shape() {
        let req = build_request(false, "abcd", "org.example.dissent", "JWT");
        assert_eq!(req.url, "https://api.push.apple.com/3/device/abcd");
        let header = |name: &str| {
            req.headers
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.as_str())
                .unwrap()
        };
        assert_eq!(header("authorization"), "bearer JWT");
        assert_eq!(header("apns-topic"), "org.example.dissent");
        assert_eq!(header("apns-push-type"), "background");
        assert_eq!(header("apns-priority"), "5");
        assert_eq!(req.body, br#"{"aps":{"content-available":1}}"#);
    }

    #[test]
    fn sandbox_uses_the_sandbox_host() {
        let req = build_request(true, "abcd", "t", "j");
        assert!(req.url.starts_with("https://api.sandbox.push.apple.com/"));
    }

    #[test]
    fn rejects_non_pem_text() {
        assert!(pkcs8_from_pem("").is_err());
        assert!(pkcs8_from_pem("-----BEGIN PRIVATE KEY-----\n!!!\n-----END PRIVATE KEY-----").is_err());
    }
}
