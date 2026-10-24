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
    fn caps_the_count() {
        let four: Vec<String> = (0..4).map(|i| format!("https://r{i}.example")).collect();
        assert!(valid_relays(&four));
        let five: Vec<String> = (0..5).map(|i| format!("https://r{i}.example")).collect();
        assert!(!valid_relays(&five));
    }
}
