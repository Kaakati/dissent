use super::*;
use dissent_identity::Identity;

fn beacon(id: &Identity, status: Status, ts_ms: u64) -> SignedPresence {
    SignedPresence::sign(id.cert.clone(), status, ts_ms, &id.device).expect("signs")
}

#[test]
fn a_signed_beacon_verifies() {
    let alice = Identity::create(1_000);
    assert!(beacon(&alice, Status::Dnd, 1_000).verify().is_ok());
}

#[test]
fn a_beacon_with_a_changed_status_is_rejected() {
    let alice = Identity::create(1_000);
    let mut sp = beacon(&alice, Status::Idle, 1_000);
    sp.status = Status::Dnd;
    assert!(sp.verify().is_err());
}

#[test]
fn a_beacon_with_a_changed_timestamp_is_rejected() {
    let alice = Identity::create(1_000);
    let mut sp = beacon(&alice, Status::Online, 1_000);
    sp.ts_ms = 2_000;
    assert!(sp.verify().is_err());
}

#[test]
fn a_beacon_signed_by_another_device_under_someone_elses_cert_is_rejected() {
    let alice = Identity::create(1_000);
    let mallory = Identity::create(1_000);
    let forged =
        SignedPresence::sign(alice.cert.clone(), Status::Online, 1_000, &mallory.device).expect("signs");
    assert!(forged.verify().is_err());
}

#[test]
fn an_offline_beacon_verifies_and_reads_as_offline() {
    let alice = Identity::create(1_000);
    let offline = beacon(&alice, Status::Invisible, 1_000);
    assert!(offline.verify().is_ok());
    assert_eq!(Presence::of(offline.status), Presence::Offline);
}

#[test]
fn a_peer_drops_out_of_the_sweep_once_its_window_ends() {
    let alice = Identity::create(1_000).root_id();
    let quiet = Identity::create(1_000).root_id();
    let offline = Identity::create(1_000).root_id();
    let heard = |status, received_ms| Heard {
        status,
        ts_ms: received_ms,
        received_ms,
    };
    let mut presence = BTreeMap::new();
    presence.insert(alice, heard(Status::Online, 10_000));
    presence.insert(quiet, heard(Status::Dnd, 10_000));
    presence.insert(offline, heard(Status::Invisible, 10_000));
    let window = millis(crate::timings::Timings::default().presence_window);
    let end = 10_000 + window;

    // Nothing ages out before the window ends.
    assert!(aged_out(&presence, window, end - 15_000, end - 1).is_empty());
    // Both beacons whose window ended in this sweep age out, once each. The offline one never does.
    let mut aged = aged_out(&presence, window, end - 1, end);
    aged.sort();
    let mut expected = vec![alice, quiet];
    expected.sort();
    assert_eq!(aged, expected);
    // A later sweep does not report them again.
    assert!(aged_out(&presence, window, end, end + 15_000).is_empty());
}

#[test]
fn a_timed_status_reads_as_online_once_it_ends() {
    let timed = OwnStatus {
        status: Status::Dnd,
        until_ms: Some(500),
    };
    assert_eq!(timed.at(499), timed);
    assert_eq!(timed.at(500), OwnStatus::default());
}

#[test]
fn invisible_reads_as_offline_to_others() {
    assert_eq!(Presence::of(Status::Invisible), Presence::Offline);
    assert_eq!(Presence::of(Status::Dnd), Presence::Dnd);
}

#[test]
fn status_strings_round_trip() {
    for status in [Status::Online, Status::Idle, Status::Dnd, Status::Invisible] {
        assert_eq!(status.as_str().parse::<Status>().unwrap(), status);
    }
    assert!("offline".parse::<Status>().is_err());
}
