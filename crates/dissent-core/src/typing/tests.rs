use super::*;
use dissent_identity::Identity;

fn scope() -> Scope {
    Scope::Conv(ConvId([7; 32]))
}

fn signal(id: &Identity, scope: Scope, stop: bool, ts_ms: u64) -> SignedTyping {
    SignedTyping::sign(id.cert.clone(), scope, stop, ts_ms, &id.device).expect("signs")
}

#[test]
fn a_signed_signal_verifies() {
    let alice = Identity::create(1_000);
    assert!(signal(&alice, scope(), false, 1_000).verify().is_ok());
}

#[test]
fn a_signal_with_a_changed_scope_is_rejected() {
    let alice = Identity::create(1_000);
    let mut st = signal(&alice, scope(), false, 1_000);
    st.scope = Scope::Conv(ConvId([8; 32]));
    assert!(st.verify().is_err());
}

#[test]
fn a_signal_turned_into_a_stop_is_rejected() {
    let alice = Identity::create(1_000);
    let mut st = signal(&alice, scope(), false, 1_000);
    st.stop = true;
    assert!(st.verify().is_err());
}

#[test]
fn a_signal_with_a_changed_timestamp_is_rejected() {
    let alice = Identity::create(1_000);
    let mut st = signal(&alice, scope(), false, 1_000);
    st.ts_ms = 2_000;
    assert!(st.verify().is_err());
}

#[test]
fn a_forged_signal_under_someone_elses_cert_is_rejected() {
    let alice = Identity::create(1_000);
    let mallory = Identity::create(1_000);
    let forged = SignedTyping::sign(alice.cert.clone(), scope(), false, 1_000, &mallory.device)
        .expect("signs");
    assert!(forged.verify().is_err());
}

#[test]
fn a_new_typer_is_announced_and_a_refresh_is_not() {
    let root = Identity::create(1_000).root_id();
    let target = TypingTarget::Dm(root);
    let mut typing = Typing::default();
    assert!(typing.start(target, root, 1_000, 1_000), "first signal changes the set");
    assert!(!typing.start(target, root, 2_000, 2_000), "a refresh does not");
}

#[test]
fn typers_are_listed_oldest_first() {
    let (a, b) = (Identity::create(1_000).root_id(), Identity::create(1_000).root_id());
    let target = TypingTarget::Group(ConvId([1; 32]));
    let mut typing = Typing::default();
    typing.start(target, b, 1_000, 1_000);
    typing.start(target, a, 1_100, 1_100);
    assert_eq!(typing.live(target, 1_200), vec![b, a]);
}

#[test]
fn a_typer_expires_six_seconds_after_the_last_signal() {
    let root = Identity::create(1_000).root_id();
    let target = TypingTarget::Dm(root);
    let mut typing = Typing::default();
    typing.start(target, root, 1_000, 1_000);
    typing.start(target, root, 4_000, 4_000);

    assert_eq!(typing.live(target, 9_999), vec![root]);
    assert!(typing.live(target, 10_000).is_empty());
    assert_eq!(typing.expire(9_999), Vec::<TypingTarget>::new());
    assert_eq!(typing.expire(10_000), vec![target]);
    assert!(typing.typers.is_empty());
}

#[test]
fn a_stop_removes_the_typer_and_a_stale_stop_does_not() {
    let root = Identity::create(1_000).root_id();
    let target = TypingTarget::Dm(root);
    let mut typing = Typing::default();
    typing.start(target, root, 5_000, 5_000);

    assert!(!typing.stop(target, root, 4_000, 5_100), "a stop older than the newest signal is ignored");
    assert!(typing.stop(target, root, 5_000, 5_100));
    assert!(typing.live(target, 5_100).is_empty());
}

#[test]
fn a_message_clears_its_author_at_once() {
    let root = Identity::create(1_000).root_id();
    let target = TypingTarget::Dm(root);
    let mut typing = Typing::default();
    typing.start(target, root, 1_000, 1_000);
    assert!(typing.remove(target, root, 1_100));
    assert!(!typing.remove(target, root, 1_100), "nothing left to clear");
}

#[test]
fn the_send_throttle_allows_one_signal_per_interval() {
    let target = TypingTarget::Group(ConvId([2; 32]));
    let mut typing = Typing::default();
    let start = Instant::now();
    assert!(typing.may_send(target, start));
    assert!(!typing.may_send(target, start + Duration::from_secs(1)));
    assert!(typing.may_send(target, start + TYPING_EVERY));
}

#[test]
fn a_dm_message_names_its_peer_and_a_group_message_names_the_group() {
    let me = Identity::create(1_000).root_id();
    let peer = Identity::create(1_000).root_id();
    let group = ConvId([9; 32]);
    assert_eq!(conv_target(&me, dm_conv_id(&me, &peer), peer), TypingTarget::Dm(peer));
    assert_eq!(conv_target(&me, group, peer), TypingTarget::Group(group));
}
