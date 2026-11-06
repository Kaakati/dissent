//! Where this device's own fetches go. Every HTTP client the engine builds, and every host name it resolves, goes
//! through here, so the device's "Hide my IP address" mode decides them all: the workspace's `clippy.toml` refuses
//! `reqwest::Client::new`, `reqwest::Client::builder` and `tokio::net::lookup_host` anywhere else.
//!
//! - [`Egress::Direct`]: fetches go straight to the host.
//! - [`Egress::Relayed`]: the device hides its IP from peers behind a relay, so a fetch would show it to the host
//!   instead. Fetches that happen on their own, link previews, are off.
//! - [`Egress::Proxied`]: fetches go through the proxy, by host name, so nothing is resolved here either.
#![allow(clippy::disallowed_methods)]

use std::net::SocketAddr;

pub use dissent_net::Egress;

/// What a fetch is for. Each mode allows some of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purpose {
    /// The page and picture of a link in a message this device sends.
    LinkPreview,
}

/// Whether `egress` allows a fetch for `purpose`. A relayed device fetches nothing on its own: the host would see
/// the IP the relay hides.
pub(crate) fn allows(egress: &Egress, purpose: Purpose) -> bool {
    match (egress, purpose) {
        (Egress::Relayed, Purpose::LinkPreview) => false,
        (Egress::Direct | Egress::Proxied(_), Purpose::LinkPreview) => true,
    }
}

/// An HTTP client for a fetch for `purpose`, set up by `configure`. `None` when `egress` does not allow the fetch.
/// A proxied one sends every request through the proxy.
pub(crate) fn http_client(
    egress: &Egress,
    purpose: Purpose,
    configure: impl FnOnce(reqwest::ClientBuilder) -> reqwest::ClientBuilder,
) -> Option<reqwest::Client> {
    if !allows(egress, purpose) {
        return None;
    }
    let builder = match egress {
        Egress::Proxied(url) => reqwest::Client::builder().proxy(reqwest::Proxy::all(url.as_str()).ok()?),
        // A proxy in the environment would see only what the host sees, but the mode says direct: none is used.
        Egress::Direct | Egress::Relayed => reqwest::Client::builder().no_proxy(),
    };
    configure(builder).build().ok()
}

/// The addresses `host` resolves to, for a check before connecting to it. `None` when it does not resolve, and
/// always for a proxied device: the proxy resolves the names, and a query from here would show the resolver which
/// hosts this device connects to.
pub(crate) async fn lookup_host(egress: &Egress, host: &str, port: u16) -> Option<Vec<SocketAddr>> {
    if matches!(egress, Egress::Proxied(_)) {
        return None;
    }
    tokio::net::lookup_host((host, port)).await.ok().map(Iterator::collect)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relayed_device_fetches_no_link_preview() {
        let proxy: url::Url = "http://127.0.0.1:9080".parse().unwrap();
        assert!(allows(&Egress::Direct, Purpose::LinkPreview));
        assert!(!allows(&Egress::Relayed, Purpose::LinkPreview), "the site would see the IP the relay hides");
        assert!(allows(&Egress::Proxied(proxy.clone()), Purpose::LinkPreview), "through the proxy, it sees the exit");
        assert!(http_client(&Egress::Relayed, Purpose::LinkPreview, |b| b).is_none());
        assert!(http_client(&Egress::Proxied(proxy), Purpose::LinkPreview, |b| b).is_some());
    }

    #[tokio::test]
    async fn a_proxied_device_resolves_no_name() {
        let proxy: url::Url = "http://127.0.0.1:9080".parse().unwrap();
        assert!(lookup_host(&Egress::Proxied(proxy), "localhost", 443).await.is_none());
        assert!(lookup_host(&Egress::Direct, "localhost", 443).await.is_some_and(|a| !a.is_empty()));
    }
}
