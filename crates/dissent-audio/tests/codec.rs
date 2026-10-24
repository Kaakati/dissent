mod common;

use common::{aligned_snr, snr_db, sweep_plus_noise, Rng};
use dissent_audio::{
    Decoder, Encoder, OpusDecoder, OpusEncoder, PcmDecoder, PcmEncoder, FRAME_SAMPLES,
};

fn encode_all(enc: &mut OpusEncoder, input: &[f32]) -> Vec<Vec<u8>> {
    input
        .as_chunks::<FRAME_SAMPLES>()
        .0
        .iter()
        .map(|f| enc.encode(f).expect("encode"))
        .collect()
}

#[test]
fn opus_roundtrip_snr_on_sweep_plus_noise() {
    let mut rng = Rng::new(7);
    let input = sweep_plus_noise(FRAME_SAMPLES * 150, &mut rng);
    let mut enc = OpusEncoder::new(32_000, 0).unwrap();
    let packets = encode_all(&mut enc, &input);

    let mut dec = OpusDecoder::new().unwrap();
    let mut out = Vec::with_capacity(input.len());
    let mut frame = vec![0.0f32; FRAME_SAMPLES];
    for p in &packets {
        assert_eq!(dec.decode(Some(p), &mut frame).unwrap(), FRAME_SAMPLES);
        out.extend_from_slice(&frame);
    }

    let (lag, snr) = aligned_snr(&input, &out, 600);
    println!("opus 32 kbps roundtrip: lag {lag} samples, SNR {snr:.1} dB");
    // VOIP mode is a speech model. Waveform SNR on this tonal sweep is about 12 dB at any bitrate.
    assert!(snr > 10.0, "SNR {snr:.1} dB too low");
}

#[test]
fn opus_roundtrip_at_low_bitrate_still_decodes_with_useful_snr() {
    let mut rng = Rng::new(9);
    let input = sweep_plus_noise(FRAME_SAMPLES * 100, &mut rng);
    let mut enc = OpusEncoder::new(16_000, 0).unwrap();
    let packets = encode_all(&mut enc, &input);
    let mut dec = OpusDecoder::new().unwrap();
    let mut out = Vec::new();
    let mut frame = vec![0.0f32; FRAME_SAMPLES];
    for p in &packets {
        dec.decode(Some(p), &mut frame).unwrap();
        out.extend_from_slice(&frame);
    }
    let (_, snr) = aligned_snr(&input, &out, 600);
    println!("opus 16 kbps roundtrip SNR {snr:.1} dB");
    assert!(snr > 8.0, "SNR {snr:.1} dB too low at 16 kbps");
}

#[test]
fn bitrate_is_clamped_to_the_supported_range() {
    let mut enc = OpusEncoder::new(1, 0).unwrap();
    enc.set_bitrate(1_000_000).unwrap();
    enc.set_bitrate(0).unwrap();
    enc.set_loss_percent(250).unwrap();
}

#[test]
fn silence_turns_into_dtx_packets() {
    let mut enc = OpusEncoder::new(24_000, 5).unwrap();
    let silence = vec![0.0f32; FRAME_SAMPLES];
    let mut lengths = Vec::new();
    for _ in 0..100 {
        lengths.push(enc.encode(&silence).unwrap().len());
    }
    let tail = &lengths[60..];
    let tiny = tail.iter().filter(|&&n| n <= 2).count();
    println!(
        "DTX: {tiny} of {} trailing silent frames were 2 bytes or less",
        tail.len()
    );
    assert!(tiny * 10 >= tail.len() * 9, "DTX did not engage: {tail:?}");
    assert!(enc.in_dtx());
}

/// Decodes the stream with frame `drop` lost and returns the error energy inside that frame,
/// comparing against the input shifted by `lag`.
fn drop_error(input: &[f32], packets: &[Vec<u8>], lag: usize, drop: usize, use_fec: bool) -> f64 {
    let mut dec = OpusDecoder::new().unwrap();
    let mut out = Vec::with_capacity(input.len());
    let mut frame = vec![0.0f32; FRAME_SAMPLES];
    for (i, p) in packets.iter().enumerate() {
        if i == drop {
            if use_fec {
                dec.decode_fec(&packets[i + 1], &mut frame).unwrap();
            } else {
                dec.decode(None, &mut frame).unwrap();
            }
        } else {
            dec.decode(Some(p), &mut frame).unwrap();
        }
        out.extend_from_slice(&frame);
    }
    let start = drop * FRAME_SAMPLES;
    let end = start + FRAME_SAMPLES;
    (start..end)
        .map(|t| {
            let r = input.get(t - lag).copied().unwrap_or(0.0);
            f64::from(out[t] - r).powi(2)
        })
        .sum()
}

#[test]
fn fec_recovers_single_dropped_packet_better_than_plc() {
    let mut rng = Rng::new(11);
    let input = sweep_plus_noise(FRAME_SAMPLES * 80, &mut rng);
    // 10 percent expected loss makes the encoder add in-band FEC.
    let mut enc = OpusEncoder::new(24_000, 10).unwrap();
    let packets = encode_all(&mut enc, &input);

    // Find the decoder delay once, from a loss-free decode.
    let mut dec = OpusDecoder::new().unwrap();
    let mut clean = Vec::new();
    let mut frame = vec![0.0f32; FRAME_SAMPLES];
    for p in &packets {
        dec.decode(Some(p), &mut frame).unwrap();
        clean.extend_from_slice(&frame);
    }
    let (lag, clean_snr) = aligned_snr(&input, &clean, 600);
    println!("clean decode: lag {lag}, SNR {clean_snr:.1} dB");

    let drops: Vec<usize> = (10..packets.len() - 2).step_by(7).collect();
    let mut plc_err = 0.0;
    let mut fec_err = 0.0;
    let mut sig = 0.0;
    for &d in &drops {
        plc_err += drop_error(&input, &packets, lag, d, false);
        fec_err += drop_error(&input, &packets, lag, d, true);
        let start = d * FRAME_SAMPLES;
        sig += (start..start + FRAME_SAMPLES)
            .map(|t| f64::from(input[t - lag]).powi(2))
            .sum::<f64>();
    }
    let plc_snr = 10.0 * (sig / plc_err).log10();
    let fec_snr = 10.0 * (sig / fec_err).log10();
    println!(
        "lost-frame SNR: PLC {plc_snr:.1} dB, FEC {fec_snr:.1} dB over {} drops",
        drops.len()
    );
    assert!(
        fec_snr > plc_snr + 3.0,
        "FEC {fec_snr:.1} dB should beat PLC {plc_snr:.1} dB by 3 dB"
    );
}

#[test]
fn pcm_codec_roundtrip_is_within_quantisation() {
    let mut rng = Rng::new(3);
    let input = sweep_plus_noise(FRAME_SAMPLES * 10, &mut rng);
    let mut enc = PcmEncoder;
    let mut dec = PcmDecoder;
    let mut out = Vec::new();
    let mut frame = vec![0.0f32; FRAME_SAMPLES];
    for f in input.as_chunks::<FRAME_SAMPLES>().0 {
        let p = enc.encode(f).unwrap();
        assert_eq!(p.len(), FRAME_SAMPLES * 2);
        dec.decode(Some(&p), &mut frame).unwrap();
        out.extend_from_slice(&frame);
    }
    assert!(snr_db(&input, &out) > 85.0);
    assert!(dec.decode_fec(&[0u8; 4], &mut frame).is_err());
}

#[test]
fn wrong_frame_length_is_rejected() {
    let mut enc = OpusEncoder::new(24_000, 0).unwrap();
    assert!(enc.encode(&[0.0; 100]).is_err());
    let mut dec = OpusDecoder::new().unwrap();
    assert!(dec.decode(None, &mut [0.0; 100]).is_err());
}
