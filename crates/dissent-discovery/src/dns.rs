//! DNS-verified handles. A user who controls `example.com` proves the handle
//! `@example.com` by putting [`dns_record_value`] in a TXT record at `_dissent.example.com`.
//! Every viewer checks this itself. A peer's claim is never trusted.

use std::time::Duration;

use dissent_identity::RootId;
use hickory_resolver::config::ResolverConfig;
use hickory_resolver::name_server::TokioConnectionProvider;
use hickory_resolver::proto::rr::{RData, RecordType};
use hickory_resolver::{ResolveError, TokioResolver};
use tracing::debug;
use unicode_security::{RestrictionLevel, RestrictionLevelDetection};

use crate::{DiscoveryError, Result};

const TXT_PREFIX: &str = "dissent-root=";
const TXT_LABEL: &str = "_dissent";
const MAX_DOMAIN_LEN: usize = 253;
const MAX_LABEL_LEN: usize = 63;
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

/// The TXT value that proves `root` owns a domain: `dissent-root=<base32 root id>`.
pub fn dns_record_value(root: &RootId) -> String {
    format!("{TXT_PREFIX}{root}")
}

/// Normalizes a domain for a handle: lowercase ASCII, punycode for IDNs, at most 253 characters.
///
/// Rejects empty and over-long labels, characters other than letters, digits and hyphens,
/// single-label names, and labels that mix scripts (for example Latin with Cyrillic
/// letters, which is a common spoof). A trailing dot is accepted and removed.
pub fn normalize_domain(domain: &str) -> Result<String> {
    let lowered = domain.trim().trim_end_matches('.').to_lowercase();
    if lowered.is_empty() {
        return Err(invalid("domain is empty"));
    }
    let ascii =
        idna::domain_to_ascii(&lowered).map_err(|e| invalid(format!("not a valid domain: {e}")))?;
    if ascii.len() > MAX_DOMAIN_LEN {
        return Err(invalid(format!(
            "domain is longer than {MAX_DOMAIN_LEN} characters"
        )));
    }
    let (unicode, _) = idna::domain_to_unicode(&ascii);
    for label in unicode.split('.') {
        if !label.check_restriction_level(RestrictionLevel::ModeratelyRestrictive) {
            return Err(invalid(format!("label `{label}` mixes scripts")));
        }
    }
    let labels: Vec<&str> = ascii.split('.').collect();
    if labels.len() < 2 {
        return Err(invalid(
            "a domain needs at least two labels, such as example.com",
        ));
    }
    for label in labels {
        check_ldh_label(label)?;
    }
    Ok(ascii)
}

fn check_ldh_label(label: &str) -> Result<()> {
    if label.is_empty() {
        return Err(invalid("domain has an empty label"));
    }
    if label.len() > MAX_LABEL_LEN {
        return Err(invalid(format!(
            "label `{label}` is longer than {MAX_LABEL_LEN} characters"
        )));
    }
    let ok_chars = label
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if !ok_chars || label.starts_with('-') || label.ends_with('-') {
        return Err(invalid(format!(
            "label `{label}` has characters a domain cannot use"
        )));
    }
    Ok(())
}

fn invalid(msg: impl Into<String>) -> DiscoveryError {
    DiscoveryError::InvalidDomain(msg.into())
}

/// True if `domain` has a TXT record at `_dissent.<domain>` whose text is
/// [`dns_record_value`]`(root)`. NXDOMAIN and "no records" return `Ok(false)`.
///
/// Uses the system resolver, falling back to Cloudflare if that fails to answer.
/// The whole check gives up after 5 seconds with an error.
pub async fn verify_dns_handle(domain: &str, root: &RootId) -> Result<bool> {
    let domain = normalize_domain(domain)?;
    let name = format!("{TXT_LABEL}.{domain}");
    let strings = tokio::time::timeout(LOOKUP_TIMEOUT, txt_with_fallback(&name))
        .await
        .map_err(|_| DiscoveryError::Dns(format!("lookup of {name} timed out")))?
        .map_err(|e| DiscoveryError::Dns(e.to_string()))?;
    let expected = dns_record_value(root);
    Ok(strings.contains(&expected))
}

/// Looks up TXT records. Each record's character-strings are concatenated, as RFC 7208 does.
async fn txt_with_fallback(name: &str) -> std::result::Result<Vec<String>, ResolveError> {
    if let Ok(builder) = TokioResolver::builder_tokio() {
        let mut builder = builder;
        builder.options_mut().timeout = LOOKUP_TIMEOUT;
        builder.options_mut().attempts = 1;
        match txt_strings(&builder.build(), name).await {
            Ok(strings) => return Ok(strings),
            Err(e) => debug!(error = %e, "system resolver failed; trying public resolver"),
        }
    }
    let public = TokioResolver::builder_with_config(
        ResolverConfig::cloudflare(),
        TokioConnectionProvider::default(),
    )
    .build();
    txt_strings(&public, name).await
}

async fn txt_strings(
    resolver: &TokioResolver,
    name: &str,
) -> std::result::Result<Vec<String>, ResolveError> {
    match resolver.lookup(name, RecordType::TXT).await {
        Ok(lookup) => Ok(lookup
            .iter()
            .filter_map(|rdata| match rdata {
                RData::TXT(txt) => {
                    let bytes: Vec<u8> = txt
                        .txt_data()
                        .iter()
                        .flat_map(|s| s.iter().copied())
                        .collect();
                    Some(String::from_utf8_lossy(&bytes).into_owned())
                }
                _ => None,
            })
            .collect()),
        Err(e) if e.is_nx_domain() || e.is_no_records_found() => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dissent_identity::RootKey;

    #[test]
    fn record_value_is_prefixed_base32_root_id() {
        let root = RootKey::generate().id();
        let value = dns_record_value(&root);
        assert_eq!(value, format!("dissent-root={root}"));
        assert_eq!(
            value
                .strip_prefix("dissent-root=")
                .unwrap()
                .parse::<RootId>()
                .unwrap(),
            root
        );
    }

    #[test]
    fn normalizes_case_and_trailing_dot() {
        assert_eq!(normalize_domain("Example.COM.").unwrap(), "example.com");
        assert_eq!(normalize_domain("  example.com ").unwrap(), "example.com");
    }

    #[test]
    fn converts_idn_to_punycode() {
        assert_eq!(
            normalize_domain("bücher.example").unwrap(),
            "xn--bcher-kva.example"
        );
    }

    #[test]
    fn rejects_spaces() {
        assert!(matches!(
            normalize_domain("exa mple.com"),
            Err(DiscoveryError::InvalidDomain(_))
        ));
    }

    #[test]
    fn rejects_empty_label() {
        assert!(matches!(
            normalize_domain("example..com"),
            Err(DiscoveryError::InvalidDomain(_))
        ));
        assert!(matches!(
            normalize_domain(".example.com"),
            Err(DiscoveryError::InvalidDomain(_))
        ));
    }

    #[test]
    fn rejects_too_long_domain() {
        let label = "a".repeat(MAX_LABEL_LEN);
        let long = format!("{label}.{label}.{label}.{label}.com");
        assert!(long.len() > MAX_DOMAIN_LEN);
        assert!(matches!(
            normalize_domain(&long),
            Err(DiscoveryError::InvalidDomain(_))
        ));
    }

    #[test]
    fn rejects_overlong_label() {
        let long = format!("{}.com", "a".repeat(64));
        assert!(matches!(
            normalize_domain(&long),
            Err(DiscoveryError::InvalidDomain(_))
        ));
    }

    #[test]
    fn rejects_mixed_script_lookalike() {
        // Cyrillic е (U+0435) inside a Latin word.
        assert!(matches!(
            normalize_domain("\u{435}xample.com"),
            Err(DiscoveryError::InvalidDomain(_))
        ));
    }

    #[test]
    fn rejects_single_label_and_bad_hyphens() {
        assert!(matches!(
            normalize_domain("localhost"),
            Err(DiscoveryError::InvalidDomain(_))
        ));
        assert!(matches!(
            normalize_domain("-example.com"),
            Err(DiscoveryError::InvalidDomain(_))
        ));
        assert!(matches!(
            normalize_domain("exa_mple.com"),
            Err(DiscoveryError::InvalidDomain(_))
        ));
    }

    /// Needs the network. Run with `scripts/fwtest.sh dissent-discovery lib --ignored dns_handle_absent_record`.
    #[ignore]
    #[tokio::test]
    async fn dns_handle_absent_record() {
        let root = RootKey::generate().id();
        assert!(!verify_dns_handle("example.com", &root).await.unwrap());
    }

    /// Needs the network. Run with `scripts/fwtest.sh dissent-discovery lib --ignored dns_handle_nxdomain`.
    #[ignore]
    #[tokio::test]
    async fn dns_handle_nxdomain() {
        let root = RootKey::generate().id();
        assert!(!verify_dns_handle("dissent-test.invalid", &root)
            .await
            .unwrap());
    }
}
