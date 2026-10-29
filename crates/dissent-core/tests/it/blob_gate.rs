//! Members-only blob fetch: a device that learned a blob's hash gets the ciphertext only if it may see what the blob
//! belongs to. See `docs/design/08-files.md`, "Who may fetch a blob".

use crate::common::*;
use dissent_core::test_support::{fetch_raw_blob, gate_forget_device};
use dissent_core::ModAction;

/// The hash and ciphertext size of an attachment, from its id and plaintext size.
fn blob_of(attachment_id: &str, size: u64) -> ([u8; 32], u64) {
    let bytes = data_encoding::BASE32_NOPAD.decode(attachment_id.to_uppercase().as_bytes()).expect("base32 id");
    (bytes.try_into().expect("32 bytes"), dissent_files::cipher_len(size))
}

fn write_file(name: &str, body: &[u8]) -> std::path::PathBuf {
    let path = scratch("blob-gate").join(name);
    std::fs::write(&path, body).expect("write source file");
    path
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn only_members_fetch_a_channel_file_and_a_ban_cuts_them_off() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let id = rustaceans(&alice, &bob).await;
        let general = channel_id(&alice.engine, id, "general").expect("general");
        let mallory = peer("mallory").await;

        let sent = alice
            .engine
            .send_files(id, general, vec![write_file("plan.txt", b"members only")], "plan".into(), true)
            .await
            .expect("alice posts a file");
        let (hash, size) = blob_of(&sent.attachments[0].id, sent.attachments[0].size);
        let from = alice.engine.my_address().expect("alice's address");

        assert_eq!(fetch_raw_blob(&bob.engine, &from, hash, size).await, Ok(()), "a member is served");
        assert_eq!(
            fetch_raw_blob(&mallory.engine, &from, hash, size).await,
            Err("refused".into()),
            "a device that only knows the hash is refused"
        );

        alice.engine.moderate(id, ModAction::Ban, bob.identity.root_id()).await.expect("alice bans bob");
        let again = alice
            .engine
            .send_files(id, general, vec![write_file("after.txt", b"after the ban")], "after".into(), true)
            .await
            .expect("alice posts another file");
        let (hash, size) = blob_of(&again.attachments[0].id, again.attachments[0].size);
        assert_eq!(fetch_raw_blob(&bob.engine, &from, hash, size).await, Err("refused".into()), "a banned member is refused");

        finish(vec![alice, bob, mallory]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_direct_file_is_for_the_conversation_and_a_private_file_for_its_readers() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let id = rustaceans(&alice, &bob).await;
        let carol = peer("carol").await;
        let invite = alice.engine.create_invite(id).await.expect("invite");
        join_ready(&alice, &carol, &invite).await;
        let from = alice.engine.my_address().expect("alice's address");

        // A direct message to Bob: Carol shares the server with Alice, not the conversation.
        let dm = alice
            .engine
            .send_dm_files(bob.identity.root_id(), vec![write_file("dm.txt", b"for bob")], String::new())
            .await
            .expect("alice sends bob a file");
        let (hash, size) = blob_of(&dm.attachments[0].id, dm.attachments[0].size);
        assert_eq!(fetch_raw_blob(&carol.engine, &from, hash, size).await, Err("refused".into()), "not in the conversation");
        assert_eq!(fetch_raw_blob(&bob.engine, &from, hash, size).await, Ok(()), "in the conversation");

        // A private channel Bob reads and Carol does not.
        let vault = alice
            .engine
            .create_private_channel(id, "vault".into(), vec![], vec![bob.identity.root_id()])
            .await
            .expect("vault");
        let vault: dissent_core::ChannelId = vault.id.parse().expect("channel id");
        let private = alice
            .engine
            .send_files(id, vault, vec![write_file("vault.txt", b"eyes only")], String::new(), true)
            .await
            .expect("alice posts in vault");
        let (hash, size) = blob_of(&private.attachments[0].id, private.attachments[0].size);
        assert_eq!(fetch_raw_blob(&carol.engine, &from, hash, size).await, Err("refused".into()), "not a reader");
        assert_eq!(fetch_raw_blob(&bob.engine, &from, hash, size).await, Ok(()), "a reader");

        finish(vec![alice, bob, carol]).await;
    })
    .await;
}

// A member the sender has not met yet: the first ask is refused, the sender asks the member's device for its
// certificate at once, and the fetch's own retry is served. No Retry from the user.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_fresh_joiner_is_served_on_the_fetch_retry() {
    bounded(async {
        let alice = peer("alice").await;
        let bob = peer("bob").await;
        let id = rustaceans(&alice, &bob).await;
        let general = channel_id(&alice.engine, id, "general").expect("general");
        let sent = alice
            .engine
            .send_files(id, general, vec![write_file("welcome.txt", b"hello newcomer")], "welcome".into(), true)
            .await
            .expect("alice posts a file");
        eventually("bob sees the file", || {
            bob.engine
                .channel_messages(id, general, 10)
                .iter()
                .any(|m| m.id == sent.id && m.attachments.len() == 1)
                .then_some(())
        })
        .await;

        gate_forget_device(&alice.engine, bob.identity.device_id());
        let started = std::time::Instant::now();
        bob.engine
            .download_attachment(Some(id), sent.id.clone(), sent.attachments[0].id.clone())
            .await
            .expect("bob starts the download");
        let local = eventually_within(std::time::Duration::from_secs(20), "bob has the file", || {
            bob.engine
                .channel_messages(id, general, 10)
                .into_iter()
                .find(|m| m.id == sent.id)
                .and_then(|m| m.attachments.first().and_then(|a| a.local_path.clone()))
        })
        .await;
        assert_eq!(std::fs::read(&local).expect("read the download"), b"hello newcomer");
        assert!(started.elapsed() >= std::time::Duration::from_millis(900), "the first ask was refused and retried");

        finish(vec![alice, bob]).await;
    })
    .await;
}
