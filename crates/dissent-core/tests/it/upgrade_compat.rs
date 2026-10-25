//! A profile written by the v0.1.4 release must still load. Unit tests write and read with the
//! same code, so they cannot catch a stored struct that changed shape. This test starts the
//! current engine on a copy of `tests/fixtures/profile-v0.1.4`, which the v0.1.4 engine wrote
//! (see that folder's README for how, and for the expected contents).

use crate::common::*;
use dissent_core::{ConvId, Engine, Identity, NodeConfig, RelayChoice, Status};
use dissent_identity::{keystore, Vault};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const PASSPHRASE: &str = "dissent fixture v0.1.4";
const SALT: [u8; 16] = *b"dissent-fixture1";

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/profile-v0.1.4")
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn offline() -> NodeConfig {
    NodeConfig {
        relay: RelayChoice::Disabled,
        loopback_only: true,
        ..NodeConfig::default()
    }
}

async fn start(identity: &Identity, dir: &Path, vault: &Arc<Vault>) -> Engine {
    let (engine, _events) = Engine::start_with_timings(
        identity.clone(),
        dir.to_path_buf(),
        offline(),
        vault.clone(),
        dissent_core::test_support::fast_timings(),
    )
    .await
    .expect("the current engine opens the v0.1.4 profile");
    engine
}

/// The contents `README.md` lists for the fixture. Each start must show the same thing.
fn assert_profile(engine: &Engine) {
    let conversations = engine.conversations();
    let dm = conversations
        .iter()
        .find(|c| c.kind == "dm")
        .expect("the DM with bob is listed");
    let group = conversations
        .iter()
        .find(|c| c.kind == "group")
        .expect("the group is listed");
    assert_eq!(group.name, "fixture crew");
    let conv: ConvId = group.id.parse().expect("group id parses");

    let communities = engine.communities();
    assert_eq!(communities.len(), 1, "one community");
    assert_eq!(communities[0].name, "Fixture Server");
    assert_eq!(communities[0].my_role, "owner");
    let view = engine
        .community_view(communities[0].id.parse().expect("community id parses"))
        .expect("the community opens");
    // Channel order is not part of the contract, so compare sorted.
    let mut channels: Vec<(&str, bool)> = view.channels.iter().map(|c| (c.name.as_str(), c.voice)).collect();
    channels.sort();
    assert_eq!(channels, [("general", false), ("lounge", true)]);
    assert_eq!(view.members.len(), 2, "alice and bob");

    let general = view
        .channels
        .iter()
        .find(|c| c.name == "general")
        .expect("general exists")
        .id
        .parse()
        .expect("channel id parses");
    let messages = engine.channel_messages(communities[0].id.parse().unwrap(), general, 100);
    assert_eq!(messages.len(), 4, "four messages in general");
    let welcome = messages
        .iter()
        .find(|m| m.text == "welcome to the fixture server")
        .expect("welcome message survives");
    assert_eq!(welcome.reactions.len(), 1, "one reaction on the welcome message");
    assert_eq!(welcome.reactions[0].emoji, "👍");
    assert_eq!(welcome.reactions[0].count, 1);
    let reply = messages
        .iter()
        .find(|m| m.text == "replying to the welcome message")
        .expect("reply survives");
    assert!(reply.reply_to.is_some(), "the reply still points at its target");

    let dm_messages = engine.dm_messages(dm.peer.parse().expect("peer parses"), 50);
    let mut dm_texts: Vec<&str> = dm_messages.iter().map(|m| m.text.as_str()).collect();
    dm_texts.sort();
    assert_eq!(dm_texts, ["dm from alice", "dm from bob"]);

    let group_texts: Vec<String> = engine
        .conversation_messages(conv, 10)
        .into_iter()
        .map(|m| m.text)
        .collect();
    assert_eq!(group_texts, ["group hello"]);

    let contacts = engine.list_contacts();
    let mut names: Vec<&str> = contacts.contacts.iter().map(|c| c.display_name.as_str()).collect();
    names.sort();
    assert_eq!(names, ["bob", "carol", "dave"]);

    assert!(matches!(engine.status().0, Status::Dnd), "status is do-not-disturb");
    assert_eq!(engine.transfer_settings().auto_accept_bytes, Some(262_144));

    assert_eq!(engine.pinned_conversations(), [format!("group:{}", group.id)]);
    assert!(
        engine.closed_conversations().contains_key(&format!("dm:{}", dm.peer)),
        "the closed DM stays closed"
    );
    assert_eq!(engine.ignored_users().len(), 1, "one ignored user");
}

#[tokio::test(flavor = "multi_thread")]
async fn v014_profile_loads_and_views_match() {
    bounded(async {
        let work = scratch("upgrade-compat");
        copy_dir(&fixture().join("engine"), &work.join("engine"));
        fs::copy(fixture().join("identity.bin"), work.join("identity.bin")).unwrap();

        let vault = Arc::new(Vault::from_passphrase(PASSPHRASE, &SALT).unwrap());
        let identity = keystore::load(&work.join("identity.bin"), &vault)
            .expect("the v0.1.4 identity opens with the fixture passphrase");

        // First start reads what v0.1.4 wrote. Later starts read what this engine wrote back.
        for run in 1..=3 {
            let engine = start(&identity, &work.join("engine"), &vault).await;
            assert_profile(&engine);
            engine
                .shutdown()
                .await
                .unwrap_or_else(|e| panic!("clean shutdown after run {run}: {e}"));
        }
        let _ = fs::remove_dir_all(&work);
    })
    .await;
}
