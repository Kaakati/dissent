//! Safety numbers: a short code two users compare to confirm they hold each
//! other's root keys. Symmetric in its arguments.

use crate::RootId;

const DOMAIN: &[u8] = b"dissent/safety-number/v1";
const GROUPS: usize = 12;

fn pair_hasher(a: &RootId, b: &RootId) -> blake3::Hasher {
    let (lo, hi) = if a.0 <= b.0 { (a, b) } else { (b, a) };
    let mut h = blake3::Hasher::new();
    h.update(DOMAIN);
    h.update(&lo.0);
    h.update(&hi.0);
    h
}

/// 32-byte digest of an unordered pair of root keys, for QR codes.
pub fn safety_fingerprint(a: &RootId, b: &RootId) -> [u8; 32] {
    *pair_hasher(a, b).finalize().as_bytes()
}

/// 60 decimal digits as 12 groups of 5, space-separated.
pub fn safety_number(a: &RootId, b: &RootId) -> String {
    let mut out = [0u8; GROUPS * 4];
    pair_hasher(a, b).finalize_xof().fill(&mut out);
    (0..GROUPS)
        .map(|i| {
            let n =
                u32::from_be_bytes([out[4 * i], out[4 * i + 1], out[4 * i + 2], out[4 * i + 3]]);
            format!("{:05}", n % 100_000)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RootKey;

    #[test]
    fn symmetric() {
        let a = RootKey::generate().id();
        let b = RootKey::generate().id();
        assert_eq!(safety_number(&a, &b), safety_number(&b, &a));
        assert_eq!(safety_fingerprint(&a, &b), safety_fingerprint(&b, &a));
    }

    #[test]
    fn different_pairs_differ() {
        let a = RootKey::generate().id();
        let b = RootKey::generate().id();
        let c = RootKey::generate().id();
        assert_ne!(safety_number(&a, &b), safety_number(&a, &c));
    }

    #[test]
    fn format_is_12_groups_of_5_digits() {
        let n = safety_number(&RootKey::generate().id(), &RootKey::generate().id());
        let groups: Vec<&str> = n.split(' ').collect();
        assert_eq!(groups.len(), 12);
        assert!(groups
            .iter()
            .all(|g| g.len() == 5 && g.bytes().all(|b| b.is_ascii_digit())));
    }
}
