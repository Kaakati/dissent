//! Links that leave the members carry only public reachability (privacy audit P-03): a long invite and a contact link
//! name the relay and public addresses, never a LAN, VPN or link-local one. On a loopback-only node the only addresses
//! are loopback, so they stay, and a long invite still joins. Run through
//! `scripts/fwtest.sh dissent-core it --ignored link_privacy::`.

use std::net::IpAddr;

use crate::common::*;
use dissent_core::CommunityId;
use dissent_discovery::{ContactCard, InviteLink};
use dissent_net::PeerAddr;

/// True when every direct address in `peers` is loopback or public.
fn only_shareable(peers: &[String]) -> bool {
    peers.iter().filter_map(|p| p.parse::<PeerAddr>().ok()).all(|a| {
        a.direct.iter().all(|s| {
            let ip: IpAddr = s.ip().to_canonical();
            ip.is_loopback() || dissent_events::relay::is_public_address(ip)
        })
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn long_invites_and_contact_links_carry_only_shareable_addresses() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let server = alice.engine.create_community("Quiet".into()).await.expect("create");
    let id: CommunityId = server.id.parse().unwrap();

    let long = alice.engine.create_full_invite(id).await.expect("long invite");
    let InviteLink::Full(invite) = InviteLink::parse(&long).expect("parses") else {
        panic!("a full invite");
    };
    assert!(!invite.peers.is_empty(), "the invite still names its member");
    assert!(only_shareable(&invite.peers), "{:?}", invite.peers);

    let card = ContactCard::parse(&alice.engine.contact_link()).expect("contact link parses");
    assert!(only_shareable(&card.peers), "{:?}", card.peers);

    // The filtered long invite still joins on loopback.
    let joined = join_ready(&alice, &bob, &long).await;
    assert_eq!(joined, id);
    finish(vec![alice, bob]).await;
    })
    .await;
}
