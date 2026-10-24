//! Open-source relay for Dissent, built on the `iroh-relay` server. A relay carries encrypted
//! traffic between two devices that cannot reach each other directly. It cannot read it.
//!
//! This crate only configures the server. The protocol lives in `iroh-relay`, pinned to the same
//! version as the `iroh` endpoint the apps use, so both sides speak one protocol.

pub mod logs;
pub mod run;
pub mod service;

use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

pub use iroh_relay::server::{Server, ServerConfig};

use iroh_relay::server::{
    AcmeConfig, CertConfig, DEFAULT_CERT_RELOAD_INTERVAL, RelayConfig, TlsConfig,
    reloading_resolver,
};

/// How the relay gets its TLS certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tls {
    /// Let's Encrypt over ACME. Needs the public DNS name and an email for the account.
    Acme {
        hostname: String,
        email: String,
        /// Use the Let's Encrypt staging directory, which issues untrusted test certificates.
        staging: bool,
        /// Where the ACME account and issued certificates are cached.
        cache_dir: PathBuf,
    },
    /// PEM certificate chain and private key on disk. Reloaded daily.
    Files { cert: PathBuf, key: PathBuf },
}

/// Plain HTTP on `bind`, with no TLS. Use it only for local testing: clients send traffic
/// unencrypted at the transport level.
pub fn dev_config(bind: SocketAddr) -> ServerConfig {
    let mut config = ServerConfig::default();
    config.relay = Some(RelayConfig::new(bind));
    config
}

/// HTTPS on `https_bind` with the given certificate. Plain HTTP stays on `bind`, where it serves
/// the captive-portal probe.
pub async fn tls_config(bind: SocketAddr, https_bind: SocketAddr, tls: Tls) -> Result<ServerConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cert = match tls {
        Tls::Acme {
            hostname,
            email,
            staging,
            cache_dir,
        } => {
            let acme = AcmeConfig::letsencrypt(!staging)
                .domains(vec![hostname])
                .contact(vec![format!("mailto:{email}")])
                .cache_path(cache_dir);
            let builder = rustls::ServerConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .context("TLS protocol versions")?
                .with_no_client_auth();
            CertConfig::LetsEncrypt {
                acme_config: acme,
                server_config_builder: builder,
            }
        }
        Tls::Files { cert, key } => {
            let resolver = reloading_resolver(
                &provider,
                cert.clone(),
                key.clone(),
                DEFAULT_CERT_RELOAD_INTERVAL,
            )
            .await
            .with_context(|| {
                format!(
                    "load certificate {} and key {}",
                    cert.display(),
                    key.display()
                )
            })?;
            let server_config = rustls::ServerConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .context("TLS protocol versions")?
                .with_no_client_auth()
                .with_cert_resolver(resolver);
            CertConfig::Manual { server_config }
        }
    };
    let mut config = ServerConfig::default();
    let mut relay = RelayConfig::new(bind);
    relay.tls = Some(TlsConfig::new(https_bind, cert));
    config.relay = Some(relay);
    Ok(config)
}

/// The relay URL to paste into a client. Dev mode prints the bound HTTP address. TLS mode prints
/// `https://<hostname>`, or the HTTPS bind address when no hostname is set.
pub fn client_url(
    server: &Server,
    hostname: Option<&str>,
    https_bind: SocketAddr,
    tls: bool,
) -> String {
    if !tls {
        let addr = server
            .http_addr()
            .map(|a| a.to_string())
            .unwrap_or_default();
        return format!("http://{addr}");
    }
    match hostname {
        Some(name) => format!("https://{name}"),
        None => format!("https://{https_bind}"),
    }
}
