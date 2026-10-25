mod common;

use common::Rng;
use dissent_audio::jitter::{JitterBuffer, Packet, Playout};
use dissent_audio::{
    Decoder, Encoder, OpusDecoder, OpusEncoder, PcmDecoder, PcmEncoder, FRAME_SAMPLES,
};

const FRAME_MS: u64 = 20;

/// Marks a frame with its sequence number, so the test can check which frame came out.
/// Values are `seq % 2000 / 2000`, which 16-bit PCM carries exactly enough to recover.
fn marked_frame(seq: u32) -> Vec<f32> {
    vec![(seq % 2000) as f32 / 2000.0; FRAME_SAMPLES]
}

/// Builds the arrival schedule: packet `i` is sent at `20 * i` ms, delayed by a uniform jitter
/// of `0..=max_jitter_ms`, and dropped with probability `loss`.
fn schedule(packets: &[Vec<u8>], max_jitter_ms: u64, loss: f64, rng: &mut Rng) -> Vec<Packet> {
    let mut arrivals = Vec::new();
    for (i, payload) in packets.iter().enumerate() {
        if rng.unit() < loss {
            continue;
        }
        let jitter = (rng.unit() * (max_jitter_ms + 1) as f64) as u64;
        arrivals.push(Packet {
            seq: i as u32,
            timestamp: i as u32 * FRAME_SAMPLES as u32,
            payload: payload.clone(),
            arrival_ms: FRAME_MS * i as u64 + jitter,
            // Opus DTX packets are one or two bytes. Speech packets are much larger.
            silent: payload.len() <= 2,
        });
    }
    arrivals.sort_by_key(|p| p.arrival_ms);
    arrivals
}

struct Run {
    kinds: Vec<Playout>,
    frames: Vec<Vec<f32>>,
    targets: Vec<f64>,
    stats: dissent_audio::jitter::JitterStats,
}

/// Runs the buffer on a 20 ms tick clock. Each tick pushes the packets that have arrived by then,
/// then pops one frame.
fn run<D: Decoder>(decoder: D, arrivals: &[Packet], ticks: usize) -> Run {
    let mut jb = JitterBuffer::new(decoder);
    let mut next = 0;
    let mut out = Run {
        kinds: Vec::new(),
        frames: Vec::new(),
        targets: Vec::new(),
        stats: Default::default(),
    };
    let mut frame = vec![0.0f32; FRAME_SAMPLES];
    for t in 0..ticks {
        let now = FRAME_MS * t as u64;
        while next < arrivals.len() && arrivals[next].arrival_ms <= now {
            jb.push(arrivals[next].clone());
            next += 1;
        }
        let kind = jb.pop(now, &mut frame).expect("pop");
        out.kinds.push(kind);
        out.frames.push(frame.clone());
        out.targets.push(jb.target_delay_ms());
    }
    out.stats = jb.stats();
    out
}

fn pcm_packets(n: usize) -> Vec<Vec<u8>> {
    let mut enc = PcmEncoder;
    (0..n as u32)
        .map(|i| enc.encode(&marked_frame(i)).unwrap())
        .collect()
}

#[test]
fn in_order_packets_play_out_unchanged() {
    let packets = pcm_packets(200);
    let arrivals: Vec<Packet> = packets
        .iter()
        .enumerate()
        .map(|(i, p)| Packet {
            seq: i as u32,
            timestamp: i as u32 * FRAME_SAMPLES as u32,
            payload: p.clone(),
            arrival_ms: FRAME_MS * i as u64,
            silent: false,
        })
        .collect();
    let r = run(PcmDecoder, &arrivals, 200);
    let decoded = r.kinds.iter().filter(|k| **k == Playout::Decoded).count();
    assert!(decoded >= 190, "only {decoded} decoded of 200");
    assert_eq!(r.stats.lost, 0);
    assert_eq!(r.stats.late, 0);
}

#[test]
fn jitter_and_loss_play_out_on_cadence_and_converge() {
    let n = 1500;
    let packets = pcm_packets(n);
    let mut rng = Rng::new(42);
    let arrivals = schedule(&packets, 80, 0.05, &mut rng);
    let ticks = n + 100;
    let r = run(PcmDecoder, &arrivals, ticks);

    // Cadence: one frame per tick, always.
    assert_eq!(r.kinds.len(), ticks);
    assert_eq!(r.frames.len(), ticks);

    // No gaps: once playout has started, no tick inside the stream is silent. The ticks after
    // the last packet are the tail, where silence is expected.
    let first = r
        .kinds
        .iter()
        .position(|k| *k != Playout::Silence)
        .expect("playout never started");
    let silent_in_stream = r.kinds[first..n]
        .iter()
        .filter(|k| **k == Playout::Silence)
        .count();
    assert_eq!(silent_in_stream, 0, "gaps after playout started");

    // Decoded frames arrive in order and carry the right content.
    let mut last: Option<i64> = None;
    for (k, f) in r.kinds.iter().zip(r.frames.iter()) {
        if *k == Playout::Decoded {
            let mark = (f[0] * 2000.0).round() as i64;
            if let Some(prev) = last {
                assert!(mark > prev, "frame {mark} after {prev}");
            }
            last = Some(mark);
        }
    }

    // Loss is concealed and accounted for.
    let s = r.stats;
    assert!(s.lost > 0, "5 percent loss should show up");
    assert_eq!(s.lost, s.concealed + s.fec_recovered + s.silenced);
    assert!(s.concealed > 0);

    // Delay converges below 120 ms and stays above the floor.
    let final_target = *r.targets.last().unwrap();
    println!(
        "jitter run: target {final_target:.1} ms, lost {}, late {}, concealed {}",
        s.lost, s.late, s.concealed
    );
    assert!(
        final_target < 120.0,
        "delay {final_target:.1} ms did not converge"
    );
    assert!(final_target >= 20.0);
}

#[test]
fn opus_stream_uses_fec_on_loss_and_keeps_cadence() {
    let n = 800;
    let mut enc = OpusEncoder::new(24_000, 10).unwrap();
    let mut rng = Rng::new(5);
    let mut src = common::voice_like(n * FRAME_SAMPLES, &mut rng);
    let packets: Vec<Vec<u8>> = src
        .as_chunks::<FRAME_SAMPLES>()
        .0
        .iter()
        .map(|f| enc.encode(f).unwrap())
        .collect();
    src.clear();
    let arrivals = schedule(&packets, 80, 0.05, &mut rng);
    let ticks = n + 100;
    let r = run(OpusDecoder::new().unwrap(), &arrivals, ticks);

    assert_eq!(r.kinds.len(), ticks);
    let s = r.stats;
    println!(
        "opus jitter run: lost {}, fec {}, concealed {}, target {:.1} ms",
        s.lost, s.fec_recovered, s.concealed, s.target_delay_ms
    );
    assert!(s.fec_recovered > 0, "FEC should recover some single losses");
    assert_eq!(s.lost, s.concealed + s.fec_recovered + s.silenced);
    assert!(s.target_delay_ms < 120.0);
}

#[test]
fn delay_grows_fast_and_shrinks_slowly() {
    // Phase 1: 4 s with no jitter. Phase 2: 4 s with 80 ms jitter. Phase 3: 20 s with no jitter.
    let mut rng = Rng::new(8);
    let mut arrivals = Vec::new();
    let mut seq = 0u32;
    let mut t_send = 0u64;
    let phases: [(u64, u64); 3] = [(200, 0), (200, 80), (1000, 0)];
    for (count, max_jitter) in phases {
        for _ in 0..count {
            let jitter = (rng.unit() * (max_jitter + 1) as f64) as u64;
            arrivals.push(Packet {
                seq,
                timestamp: seq * FRAME_SAMPLES as u32,
                payload: vec![0u8; 1920],
                arrival_ms: t_send + jitter,
                silent: false,
            });
            seq += 1;
            t_send += FRAME_MS;
        }
    }
    arrivals.sort_by_key(|p| p.arrival_ms);

    let mut jb = JitterBuffer::new(PcmDecoder);
    let mut next = 0;
    let mut frame = vec![0.0f32; FRAME_SAMPLES];
    let mut at_phase2_end = 0.0;
    let mut at_phase2_plus_1s = 0.0;
    let mut at_phase3_plus_1s = 0.0;
    let mut at_phase3_plus_20s = 0.0;
    for tick in 0..(seq as u64 + 50) {
        let now = FRAME_MS * tick;
        while next < arrivals.len() && arrivals[next].arrival_ms <= now {
            jb.push(arrivals[next].clone());
            next += 1;
        }
        jb.pop(now, &mut frame).unwrap();
        match tick {
            199 => {}
            399 => at_phase2_end = jb.target_delay_ms(),
            449 => at_phase2_plus_1s = jb.target_delay_ms(),
            599 => at_phase3_plus_1s = jb.target_delay_ms(),
            1399 => at_phase3_plus_20s = jb.target_delay_ms(),
            _ => {}
        }
    }
    println!(
        "delay: after jitter onset +1 s {at_phase2_plus_1s:.1} ms, end of jitter {at_phase2_end:.1} ms, \
         after calm +1 s {at_phase3_plus_1s:.1} ms, after calm +20 s {at_phase3_plus_20s:.1} ms"
    );
    assert!(
        at_phase2_plus_1s > 50.0,
        "target should rise fast on jitter"
    );
    assert!(
        at_phase3_plus_1s > 0.5 * at_phase2_plus_1s,
        "target should not fall at once"
    );
    assert!(
        at_phase3_plus_20s < at_phase2_plus_1s * 0.5,
        "target should fall back over time"
    );
}

#[test]
fn silent_frames_stretch_or_compress_delay() {
    // Silence keeps the sender in DTX, so packets are tiny and may be stretched or compressed.
    let n = 1200;
    let mut enc = OpusEncoder::new(24_000, 5).unwrap();
    let silence = vec![0.0f32; FRAME_SAMPLES];
    let packets: Vec<Vec<u8>> = (0..n).map(|_| enc.encode(&silence).unwrap()).collect();
    let mut rng = Rng::new(21);
    let arrivals = schedule(&packets, 80, 0.0, &mut rng);
    let r = run(OpusDecoder::new().unwrap(), &arrivals, n + 50);
    println!(
        "silence run: stretched {}, compressed {}, target {:.1} ms",
        r.stats.stretched, r.stats.compressed, r.stats.target_delay_ms
    );
    assert!(
        r.stats.stretched + r.stats.compressed > 0,
        "no delay adjustment at silence"
    );
    // Warm-up: the first packets can arrive after their slot while the estimate settles. Those
    // count as lost at their slot, and late when they arrive. Require the warm-up to be small.
    assert!(
        r.stats.lost < 50,
        "lost {} of {n} with no scheduled loss",
        r.stats.lost
    );
}
