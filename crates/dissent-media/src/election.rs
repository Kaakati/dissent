//! Peer-SFU election: which participant forwards the call, and which one is the backup.
//!
//! Every peer runs the same function on the same gossiped candidate set, so all peers reach the
//! same answer without a vote.

use std::cmp::Ordering;

/// A participant's id. It is also the MLS member index, so it is the same value everywhere.
pub type PeerId = u64;

/// Bitrate of one 720p video stream, in kbps, from the architecture spec.
pub const VIDEO_720P_KBPS: u32 = 1_500;
/// Bitrate of one forwarded audio stream, in kbps, from the architecture spec.
pub const AUDIO_STREAM_KBPS: u32 = 32;
/// Speakers an audio forwarder sends to each listener.
pub const AUDIO_TOP_SPEAKERS: u32 = 3;

/// What the forwarder carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 720p video: every participant's stream is forwarded to every other participant.
    Video720p,
    /// Audio: each listener receives the top three speakers only.
    AudioTop3,
}

/// How many participants a forwarder with `upload_kbps` of upload can serve in `mode`.
///
/// In video mode this counts the streams it can forward. In audio mode it counts the listeners,
/// each of which receives [`AUDIO_TOP_SPEAKERS`] streams.
pub fn capacity(upload_kbps: u32, mode: Mode) -> usize {
    let per_unit = match mode {
        Mode::Video720p => VIDEO_720P_KBPS,
        Mode::AudioTop3 => AUDIO_TOP_SPEAKERS * AUDIO_STREAM_KBPS,
    };
    (upload_kbps / per_unit) as usize
}

/// What the network knows about one peer who could forward the call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The peer's id.
    pub peer: PeerId,
    /// Measured upload capacity, in kbps.
    pub upload_kbps: u32,
    /// Whether the device runs on battery.
    pub on_battery: bool,
    /// Whether the device has a wired network connection.
    pub wired: bool,
    /// Whether the device has enough CPU to forward at its upload rate.
    pub cpu_ok: bool,
    /// Median round-trip time to the other participants, in milliseconds.
    pub rtt_ms_median: u32,
}

/// The forwarder and its standby, as chosen by [`elect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Election {
    /// The forwarder carrying the call now.
    pub primary: PeerId,
    /// The standby. `None` when only one candidate can serve the call.
    pub backup: Option<PeerId>,
}

/// Orders two candidates. Better forwarders come first:
///
/// 1. not on battery,
/// 2. wired,
/// 3. higher upload,
/// 4. CPU ok,
/// 5. lower median RTT,
/// 6. lower peer id, as the final tie-break.
pub fn compare(a: &Candidate, b: &Candidate) -> Ordering {
    a.on_battery
        .cmp(&b.on_battery)
        .then_with(|| b.wired.cmp(&a.wired))
        .then_with(|| b.upload_kbps.cmp(&a.upload_kbps))
        .then_with(|| b.cpu_ok.cmp(&a.cpu_ok))
        .then_with(|| a.rtt_ms_median.cmp(&b.rtt_ms_median))
        .then_with(|| a.peer.cmp(&b.peer))
}

/// The candidates able to serve `participants` in `mode`, best first.
///
/// A candidate qualifies when its [`capacity`] covers every participant. The order does not
/// depend on the order of `candidates`.
pub fn ranked(candidates: &[Candidate], participants: usize, mode: Mode) -> Vec<PeerId> {
    let mut eligible: Vec<&Candidate> = candidates
        .iter()
        .filter(|c| capacity(c.upload_kbps, mode) >= participants)
        .collect();
    eligible.sort_by(|a, b| compare(a, b));
    eligible.into_iter().map(|c| c.peer).collect()
}

/// Chooses the forwarder and its backup. Returns `None` when no candidate can serve the call.
pub fn elect(candidates: &[Candidate], participants: usize, mode: Mode) -> Option<Election> {
    let order = ranked(candidates, participants, mode);
    let primary = *order.first()?;
    Some(Election {
        primary,
        backup: order.get(1).copied(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(peer: PeerId) -> Candidate {
        Candidate {
            peer,
            upload_kbps: 10_000,
            on_battery: false,
            wired: true,
            cpu_ok: true,
            rtt_ms_median: 50,
        }
    }

    fn first_wins(a: Candidate, b: Candidate) -> PeerId {
        let order = ranked(&[a, b], 1, Mode::AudioTop3);
        order[0]
    }

    #[test]
    fn capacity_follows_the_spec_numbers() {
        // 20 Mbps divides into 13 streams of 1.5 Mbps. The spec quotes about 12, with headroom.
        assert_eq!(capacity(20_000, Mode::Video720p), 13);
        // Audio: 20 Mbps serves over 200 listeners with three 32 kbps speakers each.
        assert_eq!(capacity(20_000, Mode::AudioTop3), 208);
        assert_eq!(capacity(1_000, Mode::AudioTop3), 10);
    }

    #[test]
    fn battery_beats_every_other_factor() {
        let mut on_battery = cand(1);
        on_battery.on_battery = true;
        on_battery.upload_kbps = 500_000;
        on_battery.wired = true;
        let mut plugged = cand(2);
        plugged.upload_kbps = 1_000;
        plugged.wired = false;
        assert_eq!(first_wins(on_battery, plugged), 2);
    }

    #[test]
    fn wired_beats_upload() {
        let mut wifi = cand(1);
        wifi.wired = false;
        wifi.upload_kbps = 90_000;
        let mut cable = cand(2);
        cable.upload_kbps = 20_000;
        assert_eq!(first_wins(wifi, cable), 2);
    }

    #[test]
    fn upload_beats_cpu_and_rtt() {
        let mut big = cand(1);
        big.upload_kbps = 30_000;
        big.cpu_ok = false;
        big.rtt_ms_median = 400;
        let mut small = cand(2);
        small.upload_kbps = 20_000;
        small.cpu_ok = true;
        small.rtt_ms_median = 5;
        assert_eq!(first_wins(big, small), 1);
    }

    #[test]
    fn cpu_beats_rtt() {
        let mut slow_cpu = cand(1);
        slow_cpu.cpu_ok = false;
        slow_cpu.rtt_ms_median = 5;
        let mut fast_cpu = cand(2);
        fast_cpu.cpu_ok = true;
        fast_cpu.rtt_ms_median = 400;
        assert_eq!(first_wins(slow_cpu, fast_cpu), 2);
    }

    #[test]
    fn lower_rtt_beats_peer_id() {
        let mut far = cand(1);
        far.rtt_ms_median = 80;
        let mut near = cand(9);
        near.rtt_ms_median = 20;
        assert_eq!(first_wins(far, near), 9);
    }

    #[test]
    fn equal_candidates_break_the_tie_by_peer_id() {
        assert_eq!(first_wins(cand(7), cand(3)), 3);
    }

    #[test]
    fn ranking_ignores_input_order() {
        let mut set: Vec<Candidate> = (1..=8)
            .map(|p| {
                let mut c = cand(p);
                c.upload_kbps = 5_000 + (p as u32 % 3) * 1_000;
                c.rtt_ms_median = 10 * (p as u32 % 4);
                c
            })
            .collect();
        let forward = ranked(&set, 1, Mode::AudioTop3);
        set.reverse();
        assert_eq!(ranked(&set, 1, Mode::AudioTop3), forward);
    }

    #[test]
    fn candidates_without_enough_upload_are_ineligible() {
        let mut weak = cand(1);
        weak.upload_kbps = 500; // serves 5 listeners at most
        let strong = cand(2);
        // Twenty participants need about 1.9 Mbps of forwarding capacity.
        assert_eq!(ranked(&[weak, strong], 20, Mode::AudioTop3), vec![2]);
    }

    #[test]
    fn election_names_a_primary_and_a_backup() {
        let set = [cand(4), cand(2), cand(9)];
        let e = elect(&set, 20, Mode::AudioTop3).unwrap();
        assert_eq!(e.primary, 2);
        assert_eq!(e.backup, Some(4));
    }

    #[test]
    fn a_lone_candidate_has_no_backup() {
        let e = elect(&[cand(5)], 20, Mode::AudioTop3).unwrap();
        assert_eq!(
            e,
            Election {
                primary: 5,
                backup: None
            }
        );
    }

    #[test]
    fn no_eligible_candidate_yields_no_election() {
        let mut weak = cand(1);
        weak.upload_kbps = 100;
        assert!(elect(&[weak], 20, Mode::AudioTop3).is_none());
        assert!(elect(&[], 2, Mode::AudioTop3).is_none());
    }

    #[test]
    fn video_mode_needs_more_upload_than_audio() {
        let mut modest = cand(1);
        modest.upload_kbps = 20_000; // 13 video streams, but over 200 audio listeners
        assert!(elect(&[modest.clone()], 13, Mode::Video720p).is_some());
        assert!(elect(&[modest.clone()], 14, Mode::Video720p).is_none());
        assert!(elect(&[modest], 14, Mode::AudioTop3).is_some());
    }
}
