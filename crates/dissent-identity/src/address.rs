//! Bitcoin-style bech32m addresses (BIP-350) for public IDs.
//!
//! `dsu1…` is a user ([`RootId`](crate::RootId)), `dss1…` a server
//! ([`CommunityId`](crate::CommunityId)) and `dsd1…` a device
//! ([`DeviceId`](crate::DeviceId)). The payload is the full 32-byte key, so the
//! checksum guards typos without shortening what the key can verify.
//!
//! Addresses are a display and input form only. Stored and wire encodings stay
//! on lowercase base32 (see the crate docs).

use crate::{IdentityError, Result};

pub(crate) const USER: &str = "dsu";
pub(crate) const SERVER: &str = "dss";
pub(crate) const DEVICE: &str = "dsd";

const CHARSET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";
const BECH32M_CONST: u32 = 0x2bc8_30a3;
const CHECKSUM_LEN: usize = 6;
const MAX_LEN: usize = 90;

fn polymod(values: impl Iterator<Item = u8>) -> u32 {
    const GEN: [u32; 5] = [0x3b6a_57b2, 0x2650_8e6d, 0x1ea1_19fa, 0x3d42_33dd, 0x2a14_62b3];
    let mut chk: u32 = 1;
    for v in values {
        let top = chk >> 25;
        chk = ((chk & 0x1ff_ffff) << 5) ^ u32::from(v);
        for (i, g) in GEN.iter().enumerate() {
            if (top >> i) & 1 == 1 {
                chk ^= g;
            }
        }
    }
    chk
}

fn hrp_expand(hrp: &str) -> impl Iterator<Item = u8> + '_ {
    hrp.bytes()
        .map(|c| c >> 5)
        .chain(std::iter::once(0))
        .chain(hrp.bytes().map(|c| c & 31))
}

/// Regroup bits between widths (BIP-173 `convertbits`). With `pad` false,
/// leftover bits must be fewer than a group and zero.
fn convert_bits(data: &[u8], from: u32, to: u32, pad: bool) -> Option<Vec<u8>> {
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    let mut out = Vec::new();
    let maxv: u32 = (1 << to) - 1;
    for &v in data {
        if u32::from(v) >> from != 0 {
            return None;
        }
        acc = (acc << from) | u32::from(v);
        bits += from;
        while bits >= to {
            bits -= to;
            out.push(((acc >> bits) & maxv) as u8);
        }
    }
    if pad {
        if bits > 0 {
            out.push(((acc << (to - bits)) & maxv) as u8);
        }
    } else if bits >= from || ((acc << (to - bits)) & maxv) != 0 {
        return None;
    }
    Some(out)
}

/// Encode `payload` bytes under `hrp` as a lowercase bech32m string.
pub(crate) fn encode(hrp: &str, payload: &[u8]) -> String {
    let mut data = convert_bits(payload, 8, 5, true).unwrap_or_default();
    let pm = polymod(
        hrp_expand(hrp)
            .chain(data.iter().copied())
            .chain([0u8; CHECKSUM_LEN]),
    ) ^ BECH32M_CONST;
    for i in 0..CHECKSUM_LEN {
        data.push(((pm >> (5 * (5 - i))) & 31) as u8);
    }
    let mut s = String::with_capacity(hrp.len() + 1 + data.len());
    s.push_str(hrp);
    s.push('1');
    s.extend(data.iter().map(|&v| CHARSET[usize::from(v)] as char));
    s
}

/// Decode a bech32m string into its human-readable part and payload bytes.
/// Accepts all-lowercase or all-uppercase input and rejects mixed case.
pub(crate) fn decode(s: &str) -> Result<(String, Vec<u8>)> {
    let err = |m: &str| IdentityError::Encoding(m.to_string());
    if s.len() < CHECKSUM_LEN + 2 || s.len() > MAX_LEN {
        return Err(err("address has the wrong length"));
    }
    if s.bytes().any(|b| !(33..=126).contains(&b)) {
        return Err(err("address contains an invalid character"));
    }
    if s.bytes().any(|b| b.is_ascii_lowercase()) && s.bytes().any(|b| b.is_ascii_uppercase()) {
        return Err(err("address mixes upper and lower case"));
    }
    let s = s.to_ascii_lowercase();
    let sep = s
        .rfind('1')
        .ok_or_else(|| err("address has no '1' separator"))?;
    let (hrp, rest) = (&s[..sep], &s[sep + 1..]);
    if hrp.is_empty() {
        return Err(err("address has no prefix"));
    }
    if rest.len() < CHECKSUM_LEN {
        return Err(err("address is too short for its checksum"));
    }
    let data = rest
        .bytes()
        .map(|c| {
            CHARSET
                .iter()
                .position(|&x| x == c)
                .map(|p| p as u8)
                .ok_or_else(|| err("address contains a character outside the alphabet"))
        })
        .collect::<Result<Vec<u8>>>()?;
    if polymod(hrp_expand(hrp).chain(data.iter().copied())) != BECH32M_CONST {
        return Err(err("address checksum does not match, so it has a typo"));
    }
    let payload = convert_bits(&data[..data.len() - CHECKSUM_LEN], 5, 8, false)
        .ok_or_else(|| err("address payload is malformed"))?;
    Ok((hrp.to_string(), payload))
}

/// The error for an address whose prefix names another kind of ID.
pub(crate) fn wrong_kind(found: &str, expected: &str) -> IdentityError {
    match (kind_name(found), kind_name(expected)) {
        (Some(f), Some(e)) => IdentityError::Encoding(format!(
            "this is a {f} address, not a {e} address"
        )),
        _ => IdentityError::Encoding(format!("unknown address prefix \"{found}1\"")),
    }
}

fn kind_name(hrp: &str) -> Option<&'static str> {
    match hrp {
        USER => Some("user"),
        SERVER => Some("server"),
        DEVICE => Some("device"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // BIP-350 test vectors.
    const VALID: &[&str] = &[
        "A1LQFN3A",
        "a1lqfn3a",
        "an83characterlonghumanreadablepartthatcontainsthetheexcludedcharactersbioandnumber11sg7hg6",
        "abcdef1l7aum6echk45nj3s0wdvt2fg8x9yrzpqzd3ryx",
        "split1checkupstagehandshakeupstreamerranterredcaperredlc445v",
        "?1v759aa",
    ];

    const INVALID: &[&str] = &[
        " 1xj0phk",
        "\u{7f}1g6xzxy",
        "\u{80}1vctc34",
        "an84characterslonghumanreadablepartthatcontainsthetheexcludedcharactersbioandnumber11d6pts4",
        "qyrz8wqd2c9m",
        "1qyrz8wqd2c9m",
        "y1b0jsk6g",
        "lt1igcx5c0",
        "in1muywd",
        "mm1crxm3i",
        "au1s5cgom",
        "M1VUXWEZ",
        "16plkw9",
        "1p2gdwpf",
    ];

    #[test]
    fn bip350_valid_vectors_decode() {
        for s in VALID {
            assert!(decode(s).is_ok(), "should decode: {s}");
        }
    }

    #[test]
    fn bip350_invalid_vectors_are_refused() {
        for s in INVALID {
            assert!(decode(s).is_err(), "should refuse: {s:?}");
        }
    }

    #[test]
    fn encode_then_decode_round_trips() {
        let payload = [0xabu8; 32];
        let s = encode(USER, &payload);
        assert!(s.starts_with("dsu1"));
        assert_eq!(s.len(), 62);
        assert_eq!(decode(&s).unwrap(), (USER.to_string(), payload.to_vec()));
    }

    #[test]
    fn single_char_typo_is_detected() {
        let s = encode(USER, &[7u8; 32]);
        let mut bytes = s.into_bytes();
        let i = bytes.len() - 10;
        bytes[i] = if bytes[i] == b'q' { b'p' } else { b'q' };
        assert!(decode(std::str::from_utf8(&bytes).unwrap()).is_err());
    }

    #[test]
    fn mixed_case_is_refused() {
        let s = encode(USER, &[1u8; 32]);
        let mut mixed = s.clone();
        mixed.replace_range(0..1, "D");
        assert!(decode(&mixed).is_err());
    }

    #[test]
    fn convert_bits_rejects_nonzero_padding() {
        // 52 five-bit groups carry 260 bits; the last 4 must be zero for 32 bytes.
        let mut groups = convert_bits(&[0xffu8; 32], 8, 5, true).unwrap();
        assert_eq!(groups.len(), 52);
        *groups.last_mut().unwrap() |= 1;
        assert!(convert_bits(&groups, 5, 8, false).is_none());
    }

    #[test]
    fn wrong_kind_names_both_sides() {
        let msg = wrong_kind(SERVER, USER).to_string();
        assert!(msg.contains("this is a server address, not a user address"), "{msg}");
    }
}
