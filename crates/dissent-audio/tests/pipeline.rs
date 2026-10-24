mod common;

use common::{power, voice_like, Rng};
use dissent_audio::{
    PcmEncoder, SenderConfig, SenderPacket, VoiceReceiver, VoiceSender, FRAME_SAMPLES,
};

const TICK_MS: u64 = 20;

fn cfg(noise: bool, agc: bool) -> SenderConfig {
    SenderConfig {
        bitrate_bps: 32_000,
        loss_percent: 5,
        noise_suppression: noise,
        agc,
    }
}

/// A frame as it crosses the network: sender id, sequence, timestamp, bytes.
struct Wire {
    arrival_ms: u64,
    sender: u64,
    seq: u32,
    timestamp: u32,
    frame: Vec<u8>,
}

/// Puts each sender's packets on a path with `loss` and `max_jitter_ms` of random delay.
fn network(
    sent: &[(u64, Vec<SenderPacket>)],
    loss: f64,
    max_jitter_ms: u64,
    rng: &mut Rng,
) -> Vec<Wire> {
    let mut wire = Vec::new();
    for (sender, packets) in sent {
        for p in packets {
            if rng.unit() < loss {
                continue;
            }
            let jitter = (rng.unit() * (max_jitter_ms + 1) as f64) as u64;
            wire.push(Wire {
                arrival_ms: TICK_MS * u64::from(p.seq) + jitter,
                sender: *sender,
                seq: p.seq,
                timestamp: p.timestamp,
                frame: p.to_media_frame(),
            });
        }
    }
    wire.sort_by_key(|w| w.arrival_ms);
    wire
}

/// Frame level that carries sequence number `seq`, between 0.5 and 0.9.
fn marker(seq: u32) -> f32 {
    0.5 + 0.4 * (seq % 2000) as f32 / 2000.0
}

/// Sequence number carried by a frame level from [`marker`].
fn unmarker(v: f32) -> u32 {
    ((v - 0.5) / 0.4 * 2000.0).round() as u32
}

#[test]
fn end_to_end_pcm_frames_arrive_intact_under_loss_and_jitter() {
    // PCM frames carry their sequence number, so each output frame names the packet it came from.
    // A wrong order, a duplicate or a mismatched frame shows up exactly. The level stays above the
    // energy VAD threshold, so no frame is gated to silence.
    let frames = 300usize;
    let mut rng = Rng::new(41);
    let mut src = Vec::with_capacity(FRAME_SAMPLES * frames);
    for seq in 0..frames as u32 {
        src.extend(std::iter::repeat_n(marker(seq), FRAME_SAMPLES));
    }
    let pcm_cfg = SenderConfig {
        bitrate_bps: 32_000,
        loss_percent: 5,
        noise_suppression: false,
        agc: false,
    };
    let mut tx = VoiceSender::new(PcmEncoder, pcm_cfg);
    let packets = tx.push_capture(&src).unwrap();
    assert_eq!(packets.len(), frames);
    let sent = vec![(7u64, packets)];
    let wire = network(&sent, 0.05, 80, &mut rng);
    let arrived: std::collections::BTreeSet<u32> = wire.iter().map(|w| w.seq).collect();

    let factory =
        Box::new(|| Ok(Box::new(dissent_audio::PcmDecoder) as Box<dyn dissent_audio::Decoder>));
    let mut rx = VoiceReceiver::new(4, factory);
    let ticks = frames + 100;
    let mut next = 0;
    let mut frame = vec![0.0f32; FRAME_SAMPLES];
    let mut last_mark: Option<u32> = None;
    let mut matched = 0usize;
    for t in 0..ticks {
        let now = TICK_MS * t as u64;
        while next < wire.len() && wire[next].arrival_ms <= now {
            let w = &wire[next];
            rx.push_media_frame(w.sender, w.seq, w.timestamp, w.arrival_ms, &w.frame)
                .unwrap();
            next += 1;
        }
        rx.pop_mix(now, &mut frame).unwrap();
        let v = frame[FRAME_SAMPLES / 2];
        if v > 0.45 {
            let mark = unmarker(v);
            assert!(
                arrived.contains(&mark),
                "frame {mark} played but its packet never arrived"
            );
            if let Some(prev) = last_mark {
                assert!(mark > prev, "frame {mark} played after {prev}");
            }
            assert!(
                frame.iter().all(|s| (s - v).abs() < 1e-3),
                "frame content mixed up"
            );
            last_mark = Some(mark);
            matched += 1;
        }
    }
    let stats = rx.stats()[&7];
    println!(
        "pcm e2e: {matched} frames matched, lost {}, concealed {}, target {:.1} ms",
        stats.lost, stats.concealed, stats.target_delay_ms
    );
    assert!(
        matched + stats.lost as usize >= frames - 2,
        "frames unaccounted for"
    );
    assert!(
        stats.lost > 0 && stats.lost < 60,
        "lost {} of {frames}",
        stats.lost
    );
}

#[test]
fn end_to_end_opus_keeps_the_signal_level_under_loss_and_jitter() {
    // Opus waveform SNR is unreliable as an end-to-end check. The period of a sweep can align
    // with the wrong lag, and the playout delay drifts. The level of the speech is lag-free, so
    // compare power over the whole run and over half-second chunks.
    let frames = 300;
    let mut rng = Rng::new(31);
    let src = voice_like(FRAME_SAMPLES * frames, &mut rng);

    let mut tx = VoiceSender::opus(cfg(false, false)).unwrap();
    let packets = tx.push_capture(&src).unwrap();
    assert_eq!(packets.len(), frames);
    let sent = vec![(7u64, packets)];
    let wire = network(&sent, 0.05, 80, &mut rng);

    let mut rx = VoiceReceiver::opus(4);
    let ticks = frames + 100;
    let mut out_all = Vec::with_capacity(ticks * FRAME_SAMPLES);
    let mut next = 0;
    let mut frame = vec![0.0f32; FRAME_SAMPLES];
    for t in 0..ticks {
        let now = TICK_MS * t as u64;
        while next < wire.len() && wire[next].arrival_ms <= now {
            let w = &wire[next];
            rx.push_media_frame(w.sender, w.seq, w.timestamp, w.arrival_ms, &w.frame)
                .unwrap();
            next += 1;
        }
        rx.pop_mix(now, &mut frame).unwrap();
        out_all.extend_from_slice(&frame);
    }

    let stats = rx.stats()[&7];
    let in_db = 10.0 * power(&src).log10();
    let out_db = 10.0 * power(&out_all[..src.len()]).log10();
    println!(
        "opus e2e level: input {in_db:.1} dB, output {out_db:.1} dB, lost {}, fec {}, concealed {},          stretched {}, compressed {}, target {:.1} ms",
        stats.lost, stats.fec_recovered, stats.concealed, stats.stretched, stats.compressed,
        stats.target_delay_ms
    );
    assert!(
        (out_db - in_db).abs() < 2.0,
        "output level off by {:.1} dB",
        out_db - in_db
    );

    let chunk = 24_000;
    for (i, c) in src.chunks_exact(chunk).enumerate().skip(1) {
        let a = 10.0 * power(c).log10();
        let b = 10.0 * power(&out_all[i * chunk..(i + 1) * chunk]).log10();
        assert!(
            (a - b).abs() < 6.0,
            "chunk {i}: input {a:.1} dB, output {b:.1} dB"
        );
    }
    assert!(stats.fec_recovered > 0);
    assert!(stats.target_delay_ms < 120.0);
}

#[test]
fn mute_sends_digital_silence_as_dtx_packets() {
    let mut rng = Rng::new(4);
    let src = voice_like(FRAME_SAMPLES * 120, &mut rng);
    let mut tx = VoiceSender::opus(cfg(false, false)).unwrap();
    tx.set_muted(true);
    assert!(tx.is_muted());
    let packets = tx.push_capture(&src).unwrap();
    assert!(packets.iter().all(|p| !p.voiced));
    assert!(packets.iter().all(|p| p.audio_level == 127));
    // libopus sends a full update about every 400 ms, so most frames, not all, are DTX.
    let tail = &packets[60..];
    let tiny = tail.iter().filter(|p| p.payload.len() <= 2).count();
    assert!(
        tiny * 10 >= tail.len() * 9,
        "muted frames should be DTX, {tiny} of {}",
        tail.len()
    );
}

#[test]
fn push_to_talk_gates_audio_on_the_key() {
    let mut rng = Rng::new(5);
    let src = voice_like(FRAME_SAMPLES * 60, &mut rng);
    let mut tx = VoiceSender::opus(cfg(false, false)).unwrap();
    tx.set_push_to_talk(true);
    assert!(!tx.transmitting());
    let released = tx.push_capture(&src).unwrap();
    assert!(released.iter().all(|p| !p.voiced));

    tx.set_ptt_pressed(true);
    assert!(tx.transmitting());
    let pressed = tx.push_capture(&src).unwrap();
    assert!(
        pressed.iter().any(|p| p.voiced),
        "held key should send speech"
    );
}

#[test]
fn audio_level_is_loud_for_speech_and_silent_for_silence() {
    let mut rng = Rng::new(6);
    let speech = voice_like(FRAME_SAMPLES * 60, &mut rng);
    let mut tx = VoiceSender::opus(cfg(false, false)).unwrap();
    let speech_packets = tx.push_capture(&speech).unwrap();
    let voiced: Vec<u8> = speech_packets
        .iter()
        .filter(|p| p.voiced)
        .map(|p| p.audio_level)
        .collect();
    assert!(!voiced.is_empty());
    assert!(voiced.iter().all(|&l| l < 127));

    let mut tx = VoiceSender::opus(cfg(false, false)).unwrap();
    let silence = vec![0.0f32; FRAME_SAMPLES * 60];
    let quiet = tx.push_capture(&silence).unwrap();
    assert!(quiet.iter().all(|p| p.audio_level == 127 && !p.voiced));
}

#[test]
fn receiver_mixes_only_the_loudest_senders() {
    let frames = 40;
    let loud = vec![0.5f32; FRAME_SAMPLES];
    let quiet = vec![0.1f32; FRAME_SAMPLES];

    let make = |amp: &[f32]| {
        let mut tx = VoiceSender::new(PcmEncoder, cfg(false, false));
        let mut packets = Vec::new();
        for _ in 0..frames {
            packets.extend(tx.push_capture(amp).unwrap());
        }
        packets
    };
    let loud_packets = make(&loud);
    let quiet_packets = make(&quiet);

    let run = |max_mixed: usize| {
        let factory =
            Box::new(|| Ok(Box::new(dissent_audio::PcmDecoder) as Box<dyn dissent_audio::Decoder>));
        let mut rx = VoiceReceiver::new(max_mixed, factory);
        let mut frame = vec![0.0f32; FRAME_SAMPLES];
        let mut mean = 0.0f32;
        let mut count = 0;
        for t in 0..frames as u64 {
            let now = TICK_MS * t;
            for (id, packets) in [(1u64, &loud_packets), (2u64, &quiet_packets)] {
                if let Some(p) = packets.iter().find(|p| u64::from(p.seq) * TICK_MS == now) {
                    rx.push_media_frame(id, p.seq, p.timestamp, now, &p.to_media_frame())
                        .unwrap();
                }
            }
            rx.pop_mix(now, &mut frame).unwrap();
            // Skip the start-up ticks, then average the steady-state mix.
            if t >= 5 && t < frames as u64 - 5 {
                mean += frame.iter().sum::<f32>() / FRAME_SAMPLES as f32;
                count += 1;
            }
        }
        mean / count as f32
    };

    let one_mean = run(1);
    let two_mean = run(2);
    println!("mixed level: one sender {one_mean:.3}, two senders {two_mean:.3}");
    assert!(
        (one_mean - 0.5).abs() < 0.02,
        "max_mixed=1 should keep only the loud sender"
    );
    assert!(
        (two_mean - 0.6).abs() < 0.02,
        "max_mixed=2 should sum both senders"
    );
}
