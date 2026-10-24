//! Which addresses a push endpoint may point at. Without this, a registration could make the
//! relay send requests into the local network or to a cloud metadata service (SSRF).

use crate::{Config, PushError};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use url::{Host, Url};

/// True for addresses a public push provider can have. False for loopback, private, link-local,
/// shared (CGNAT), multicast, documentation and unspecified ranges.
pub fn ip_is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => ipv4_is_public(v4),
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return ipv4_is_public(mapped);
            }
            ipv6_is_public(v6)
        }
    }
}

fn ipv4_is_public(ip: Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    let shared = a == 100 && (64..128).contains(&b);
    let documentation = matches!(ip.octets(), [192, 0, 2, _] | [198, 51, 100, _] | [203, 0, 113, _]);
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_unspecified()
        || ip.is_multicast()
        || shared
        || documentation
        || a == 0)
}

fn ipv6_is_public(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    let link_local = first & 0xffc0 == 0xfe80;
    let unique_local = first & 0xfe00 == 0xfc00;
    let documentation = first == 0x2001 && ip.segments()[1] == 0x0db8;
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || link_local
        || unique_local
        || documentation)
}

/// A checked UnifiedPush endpoint. When the host is a name, its addresses are pinned, so the
/// request goes to the addresses that were checked, not to a fresh DNS answer.
#[derive(Debug, Clone)]
pub struct Target {
    pub url: Url,
    /// Host name and the checked addresses for it. `None` when the URL holds an IP literal.
    pub pin: Option<(String, Vec<SocketAddr>)>,
}

/// Checks a UnifiedPush endpoint URL. Requires https, except in dev mode. Refuses private
/// addresses unless dev mode is on. With an allow list, the host must be on it.
pub async fn check_unifiedpush_url(raw: &str, cfg: &Config) -> Result<Target, PushError> {
    let url = Url::parse(raw).map_err(|e| PushError::InvalidEndpoint(format!("bad URL: {e}")))?;
    match url.scheme() {
        "https" => {}
        "http" if cfg.dev => {}
        _ => return Err(PushError::InvalidEndpoint("endpoint must use https".into())),
    }
    let host = url
        .host()
        .ok_or_else(|| PushError::InvalidEndpoint("endpoint has no host".into()))?;
    let host_text = url.host_str().unwrap_or_default().to_string();
    if !cfg.ups_allow_hosts.is_empty()
        && !cfg
            .ups_allow_hosts
            .iter()
            .any(|h| h.eq_ignore_ascii_case(&host_text))
    {
        return Err(PushError::InvalidEndpoint(format!(
            "host {host_text} is not on the allow list"
        )));
    }
    let port = url
        .port_or_known_default()
        .ok_or_else(|| PushError::InvalidEndpoint("endpoint has no port".into()))?;
    match host {
        Host::Ipv4(ip) => {
            check_ip(IpAddr::V4(ip), cfg)?;
            Ok(Target { url, pin: None })
        }
        Host::Ipv6(ip) => {
            check_ip(IpAddr::V6(ip), cfg)?;
            Ok(Target { url, pin: None })
        }
        Host::Domain(name) => {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((name, port))
                .await
                .map_err(|e| PushError::InvalidEndpoint(format!("cannot resolve host: {e}")))?
                .collect();
            if addrs.is_empty() {
                return Err(PushError::InvalidEndpoint("host resolves to no address".into()));
            }
            for addr in &addrs {
                check_ip(addr.ip(), cfg)?;
            }
            Ok(Target {
                url: url.clone(),
                pin: Some((name.to_string(), addrs)),
            })
        }
    }
}

fn check_ip(ip: IpAddr, cfg: &Config) -> Result<(), PushError> {
    if cfg.dev || ip_is_public(ip) {
        Ok(())
    } else {
        Err(PushError::InvalidEndpoint(format!(
            "address {ip} is not a public address"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_addresses() {
        let public = ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"];
        let private = [
            "10.0.0.5",
            "192.168.1.1",
            "172.16.4.4",
            "127.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fe80::1",
            "fd00::1",
            "::ffff:10.1.2.3",
            "ff02::1",
        ];
        for p in public {
            assert!(ip_is_public(p.parse().unwrap()), "{p} should be public");
        }
        for p in private {
            assert!(!ip_is_public(p.parse().unwrap()), "{p} should be refused");
        }
    }

    fn cfg(dev: bool, allow: &[&str]) -> Config {
        Config {
            dev,
            ups_allow_hosts: allow.iter().map(|s| s.to_string()).collect(),
            snapshot: None,
        }
    }

    #[tokio::test]
    async fn refuses_private_literal_addresses_without_dev() {
        for url in ["https://10.0.0.5/up", "https://169.254.169.254/latest", "https://[fe80::1]/x"] {
            let err = check_unifiedpush_url(url, &cfg(false, &[])).await.unwrap_err();
            assert!(matches!(err, PushError::InvalidEndpoint(_)), "{url}: {err}");
        }
    }

    #[tokio::test]
    async fn refuses_plain_http_without_dev() {
        let err = check_unifiedpush_url("http://8.8.8.8/up", &cfg(false, &[]))
            .await
            .unwrap_err();
        assert!(matches!(err, PushError::InvalidEndpoint(_)));
    }

    #[tokio::test]
    async fn dev_allows_loopback_and_http() {
        let target = check_unifiedpush_url("http://127.0.0.1:9/up", &cfg(true, &[]))
            .await
            .unwrap();
        assert!(target.pin.is_none());
    }

    #[tokio::test]
    async fn allow_list_limits_hosts() {
        let allowed = cfg(false, &["push.example.org"]);
        let err = check_unifiedpush_url("https://other.example.net/up", &allowed)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("allow list"), "{err}");
    }
}
