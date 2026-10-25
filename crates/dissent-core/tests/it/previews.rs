//! Link previews: off by default, and a link to a private address gets no preview.


use crate::common::*;

#[tokio::test(flavor = "multi_thread")]
async fn previews_are_off_by_default_and_fetch_nothing() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let community = rustaceans(&alice, &bob).await;
    let general = channel_id(&alice.engine, community, "general").expect("general channel");
    assert!(!alice.engine.link_previews());

    let sent = alice
        .engine
        .send_message(community, general, "see https://example.com/page".into())
        .await
        .expect("alice posts");
    assert!(sent.embeds.is_empty(), "no preview unless the sender turns them on");
    assert_eq!(sent.text, "see https://example.com/page");

    finish(vec![alice, bob]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_to_a_private_address_gets_no_preview() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let community = rustaceans(&alice, &bob).await;
    let general = channel_id(&alice.engine, community, "general").expect("general channel");
    alice.engine.set_link_previews(true).expect("previews on");

    // localhost resolves to a loopback address, which the preview fetch refuses.
    let sent = alice
        .engine
        .send_message(community, general, "local https://localhost/admin".into())
        .await
        .expect("alice posts");
    assert!(sent.embeds.is_empty(), "a private address gets no preview");
    assert_eq!(sent.text, "local https://localhost/admin");

    finish(vec![alice, bob]).await;
}
