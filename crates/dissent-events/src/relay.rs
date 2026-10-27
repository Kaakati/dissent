//! Relay servers a community asks its members to use. Only https URLs are accepted, and a
//! community may name a few. Each relay's certificate is checked when a member dials it.

/// Most relay servers one community may name.
pub const MAX_COMMUNITY_RELAYS: usize = 4;
/// Longest relay URL accepted.
pub const MAX_RELAY_URL_LEN: usize = 256;

/// Whether `s` is a plausible https relay URL: a scheme and host, no whitespace or control
/// characters, and not too long.
pub fn valid_relay_url(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("https://") else {
        return false;
    };
    s.len() <= MAX_RELAY_URL_LEN
        && !rest.is_empty()
        && !rest.starts_with('/')
        && !s.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Whether an address a relay check may dial is public. Loopback, private, link-local, unspecified,
/// multicast, documentation and shared (carrier-grade NAT) addresses are not, so a check cannot be
/// pointed at a member's own network. An IPv4-mapped IPv6 address is judged as its IPv4 address.
pub fn is_public_address(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            let shared = o[0] == 100 && (o[1] & 0xc0) == 64;
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_broadcast()
                || v4.is_documentation()
                || o[0] == 0
                || shared)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_address(IpAddr::V4(v4));
            }
            let s = v6.segments();
            let unique_local = (s[0] & 0xfe00) == 0xfc00;
            let link_local = (s[0] & 0xffc0) == 0xfe80;
            let documentation = s[0] == 0x2001 && s[1] == 0x0db8;
            !(v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || unique_local || link_local || documentation)
        }
    }
}

/// The host of an https relay URL, without brackets or port. `None` when `s` is not one.
fn relay_host(s: &str) -> Option<&str> {
    let rest = s.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    if let Some(v6) = authority.strip_prefix('[') {
        return v6.split_once(']').map(|(host, _)| host);
    }
    Some(authority.split_once(':').map_or(authority, |(host, _)| host))
}

/// Names that resolve only on a local network or this machine.
const LOCAL_SUFFIXES: &[&str] = &[".localhost", ".local", ".internal", ".lan", ".home.arpa", ".localdomain"];

/// Whether `s` names a relay that could be public, judged from the text alone, so replay and every
/// app agree: an IP literal must be public, and a name must have a dot and not be local (`localhost`,
/// `.local`, `.internal`, `.lan`, `.home.arpa`). A name can still resolve to a private address, so
/// a member checks the resolved address too before it connects.
pub fn public_relay_host(s: &str) -> bool {
    let Some(host) = relay_host(s) else {
        return false;
    };
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return is_public_address(ip);
    }
    let name = host.trim_end_matches('.').to_ascii_lowercase();
    !name.is_empty()
        && name.contains('.')
        && name != "localhost"
        && !LOCAL_SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
        // Numbers like 3232235777 or 0x7f.1 are IPv4 addresses to most resolvers.
        && !name.split('.').all(numeric_label)
}

/// Whether a host name label is a number, decimal or `0x` hex, as IPv4 parsers read them.
fn numeric_label(label: &str) -> bool {
    match label.strip_prefix("0x") {
        Some(hex) => hex.chars().all(|c| c.is_ascii_hexdigit()),
        None => !label.is_empty() && label.chars().all(|c| c.is_ascii_digit()),
    }
}

/// Whether `s` is the loopback relay of a test or a `DISSENT_LOOPBACK` run: `127.0.0.1`, `::1` or
/// `localhost`. Only a device that runs on loopback alone accepts one.
pub fn loopback_relay_host(s: &str) -> bool {
    let Some(host) = relay_host(s) else {
        return false;
    };
    host.eq_ignore_ascii_case("localhost") || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Whether a relay list is acceptable as a whole: at most `MAX_COMMUNITY_RELAYS` valid URLs.
pub fn valid_relays(urls: &[String]) -> bool {
    urls.len() <= MAX_COMMUNITY_RELAYS && urls.iter().all(|u| valid_relay_url(u))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_https_and_refuses_the_rest() {
        assert!(valid_relay_url("https://relay.example.org"));
        assert!(valid_relay_url("https://relay.example.org:8443/"));
        assert!(!valid_relay_url("http://relay.example.org"));
        assert!(!valid_relay_url("https://"));
        assert!(!valid_relay_url("https:///x"));
        assert!(!valid_relay_url("https://a b.example"));
        let long = format!("https://{}", "a".repeat(MAX_RELAY_URL_LEN));
        assert!(!valid_relay_url(&long));
    }

    #[test]
    fn local_hosts_are_not_public() {
        for local in [
            "https://192.168.1.1",
            "https://10.0.0.2:443/",
            "https://127.0.0.1:3340",
            "https://[::1]:443",
            "https://[fd00::1]/",
            "https://169.254.1.1",
            "https://localhost",
            "https://LOCALHOST:8443/",
            "https://relay.localhost",
            "https://nas.local",
            "https://router.lan",
            "https://box.home.arpa",
            "https://intranet",
            "https://3232235777",
            "https://0x7f.1",
            "https://user@192.168.1.1/",
        ] {
            assert!(!public_relay_host(local), "{local} is local");
        }
        for public in ["https://relay.example.org", "https://relay.example.org:8443/x", "https://8.8.8.8", "https://[2606:4700::1111]/"] {
            assert!(public_relay_host(public), "{public} could be public");
        }
    }

    #[test]
    fn loopback_hosts_are_recognised() {
        assert!(loopback_relay_host("https://127.0.0.1:5000/"));
        assert!(loopback_relay_host("https://[::1]:5000"));
        assert!(loopback_relay_host("https://localhost"));
        assert!(!loopback_relay_host("https://192.168.1.1"));
        assert!(!loopback_relay_host("https://relay.example.org"));
    }

    #[test]
    fn caps_the_count() {
        let four: Vec<String> = (0..4).map(|i| format!("https://r{i}.example")).collect();
        assert!(valid_relays(&four));
        let five: Vec<String> = (0..5).map(|i| format!("https://r{i}.example")).collect();
        assert!(!valid_relays(&five));
    }
}
