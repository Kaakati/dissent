//! Which codecs a call can use, and the preference order between them.

/// A video codec the crate can encode and decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// AV1, encoded with SVT-AV1 and decoded with dav1d's Rust port, rav1d.
    Av1,
    /// H.264, encoded and decoded with OpenH264. The fallback for peers without AV1.
    H264,
}

impl Codec {
    /// Every codec this build can decode, most preferred first.
    pub const ALL: [Codec; 2] = [Codec::Av1, Codec::H264];

    /// Codecs this build can encode, most preferred first. AV1 encoding needs the `svt-av1`
    /// feature, so without it a call offers H.264 only. AV1 is still decoded.
    #[cfg(feature = "svt-av1")]
    pub const ENCODABLE: &'static [Codec] = &[Codec::Av1, Codec::H264];

    /// Codecs this build can encode, most preferred first. AV1 encoding needs the `svt-av1`
    /// feature, so without it a call offers H.264 only. AV1 is still decoded.
    #[cfg(not(feature = "svt-av1"))]
    pub const ENCODABLE: &'static [Codec] = &[Codec::H264];
}

/// The codecs both sides can use, in `ours` order. AV1 comes first whenever both support it,
/// so an empty result means the peers share no codec and cannot call.
pub fn negotiate(ours: &[Codec], theirs: &[Codec]) -> Vec<Codec> {
    ours.iter()
        .copied()
        .filter(|codec| theirs.contains(codec))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_av1_when_both_sides_can() {
        let common = negotiate(&Codec::ALL, &[Codec::H264, Codec::Av1]);
        assert_eq!(common.first(), Some(&Codec::Av1));
        assert_eq!(common, vec![Codec::Av1, Codec::H264]);
    }

    #[test]
    fn falls_back_to_h264_without_av1() {
        let common = negotiate(&Codec::ALL, &[Codec::H264]);
        assert_eq!(common, vec![Codec::H264]);
    }

    #[test]
    fn no_shared_codec_means_no_call() {
        assert!(negotiate(&[Codec::Av1], &[Codec::H264]).is_empty());
    }
}
