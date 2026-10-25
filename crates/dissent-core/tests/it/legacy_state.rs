//! A device upgraded from the plaintext version: its MLS snapshot and content log are plaintext on
//! disk. Opening it under a vault must migrate both, keep the messages readable, remove the
//! plaintext files, and leave no plaintext marker in the sealed files.


use crate::common::*;
use dissent_core::{ChannelId, CommunityId, Engine, Identity};
use dissent_identity::Vault;
use std::path::Path;

/// The purposes and labels the sealed files are written under. They match dissent-mls and core.
const MLS_PURPOSE: &[u8] = b"dissent/mls-state/v1";
const MLS_AAD: &[u8] = b"dissent/mls-state.bin";
const CONTENT_AAD: &[u8] = b"dissent/content-log.sealed";
const MESSAGE: &str = "written before the upgrade";

fn content_vault(vault: &Vault, community: &CommunityId) -> Vault {
    vault.subkey(&[b"dissent/content-log/v1".as_slice(), community.0.as_slice()].concat())
}

fn channel_id(engine: &Engine, community: CommunityId, name: &str) -> Option<ChannelId> {
    engine
        .community_view(community)
        .ok()?
        .channels
        .iter()
        .find(|c| c.name == name)
        .and_then(|c| c.id.parse().ok())
}

fn has_message(engine: &Engine, community: CommunityId, channel: ChannelId, text: &str) -> bool {
    engine
        .channel_messages(community, channel, 100)
        .iter()
        .any(|m| m.text == text)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

/// Rewrites the sealed MLS snapshot and content log of a stopped device as the plaintext files the
/// previous version wrote. The plaintext contents are the same bytes that were sealed.
fn downgrade_to_plaintext(dir: &Path, id: CommunityId) {
    let vault = dissent_core::test_support::vault_for(dir);

    let sealed_mls = std::fs::read(dir.join("mls.bin")).expect("mls.bin is written on shutdown");
    let plain_mls = vault
        .subkey(MLS_PURPOSE)
        .open_sealed(MLS_AAD, &sealed_mls)
        .expect("mls.bin opens under its vault");
    assert!(plain_mls.starts_with(b"DSNP"), "the plaintext snapshot has the old header");
    assert!(
        contains(&plain_mls, b"init_secret"),
        "the plaintext snapshot holds key material as text, so the check below means something"
    );
    std::fs::write(dir.join("mls.bin"), &plain_mls).unwrap();

    let community_dir = dir.join("communities").join(id.to_string());
    let sealed_log = std::fs::read(community_dir.join("content.sealed"))
        .expect("the decrypted content log exists before the downgrade");
    let key = content_vault(&vault, &id);
    let mut plain_log = Vec::new();
    let mut pos = 0usize;
    while pos + 4 <= sealed_log.len() {
        let len = u32::from_be_bytes(sealed_log[pos..pos + 4].try_into().unwrap()) as usize;
        let body = key
            .open_sealed(CONTENT_AAD, &sealed_log[pos + 4..pos + 4 + len])
            .expect("content record opens under its vault");
        plain_log.extend_from_slice(&(body.len() as u32).to_be_bytes());
        plain_log.extend_from_slice(&body);
        pos += 4 + len;
    }
    std::fs::write(community_dir.join("content.log"), &plain_log).unwrap();
    std::fs::remove_file(community_dir.join("content.sealed")).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plaintext_mls_and_content_log_migrate_on_open() {
    bounded(async {
        let alice = start(Identity::create(now()), scratch("legacy")).await;
        let community = alice
            .engine
            .create_community("Legacy".into())
            .await
            .expect("create community");
        let id: CommunityId = community.id.parse().unwrap();
        let general = channel_id(&alice.engine, id, "general").expect("general channel");
        alice
            .engine
            .send_message(id, general, MESSAGE.into())
            .await
            .expect("alice posts");
        assert!(has_message(&alice.engine, id, general, MESSAGE));

        let (identity, dir) = stop(alice).await;
        downgrade_to_plaintext(&dir, id);
        let community_dir = dir.join("communities").join(id.to_string());
        assert!(community_dir.join("content.log").exists());

        // Opening under the vault migrates both files.
        let upgraded = start(identity, dir.clone()).await;
        assert!(
            has_message(&upgraded.engine, id, general, MESSAGE),
            "the message written before the upgrade is still readable"
        );
        assert!(
            !community_dir.join("content.log").exists(),
            "the plaintext content log is removed"
        );
        let sealed_log = std::fs::read(community_dir.join("content.sealed"))
            .expect("the sealed content log is written");
        assert!(
            !contains(&sealed_log, MESSAGE.as_bytes()),
            "the sealed content log holds no plaintext message"
        );

        let mls = std::fs::read(dir.join("mls.bin")).unwrap();
        assert!(!mls.starts_with(b"DSNP"), "the MLS snapshot is sealed after migration");
        assert!(!contains(&mls, b"init_secret"), "the sealed MLS snapshot holds no plaintext");

        // The migrated MLS state still works: a new message is sealed and readable.
        upgraded
            .engine
            .send_message(id, general, "after the upgrade".into())
            .await
            .expect("alice posts after the upgrade");
        assert!(has_message(&upgraded.engine, id, general, "after the upgrade"));

        let (_identity, _dir) = stop(upgraded).await;
        let _ = std::fs::remove_dir_all(&dir);
    })
    .await;
}
