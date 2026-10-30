//! Colour profiles: the colour data stays, and the text that can name a device or a person goes. A profile made by a
//! calibration tool is often called after the display or its owner ("Maya's MacBook Pro"), and its copyright and device
//! description tags name the maker and model. The header's device fields, creator, date and profile id are cleared too
//! (a zero id means "not computed", which the format allows).

/// Tags whose text is cleared: the description, the copyright, and the device maker and model descriptions.
const TEXT_TAGS: [&[u8; 4]; 4] = [b"desc", b"cprt", b"dmnd", b"dmdd"];

/// The byte ranges of `profile` to zero, as `(start, len)`. Empty when it is not a profile this can read.
pub(super) fn text_ranges(profile: &[u8]) -> Vec<(usize, usize)> {
    if profile.len() < 132 || profile.get(36..40) != Some(b"acsp") {
        return Vec::new();
    }
    // Header: date and time (24..36), device manufacturer and model (48..56), creator (80..84), profile id (84..100).
    let mut out = vec![(24, 12), (48, 8), (80, 20)];
    let count = super::be32(profile, 128).unwrap_or(0).min(256) as usize;
    for i in 0..count {
        let entry = 132 + i * 12;
        let (Some(sig), Some(offset), Some(size)) = (profile.get(entry..entry + 4), super::be32(profile, entry + 4), super::be32(profile, entry + 8)) else {
            break;
        };
        let (offset, size) = (offset as usize, size as usize);
        // The tag's own type signature and reserved word stay, so the empty text still parses.
        if TEXT_TAGS.iter().any(|t| t.as_slice() == sig) && size > 8 && offset.saturating_add(size) <= profile.len() {
            out.push((offset + 8, size - 8));
        }
    }
    out
}

/// Clears the text of `profile` in place.
pub(super) fn scrub(profile: &mut [u8]) {
    for (start, len) in text_ranges(profile) {
        profile[start..start + len].fill(0);
    }
}
