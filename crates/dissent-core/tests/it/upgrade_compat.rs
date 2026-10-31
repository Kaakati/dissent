//! A profile written by the v0.1.4 release must still load. Unit tests write and read with the
//! same code, so they cannot catch a stored struct that changed shape. This test starts the
//! current engine on a copy of `tests/fixtures/profile-v0.1.4`, which the v0.1.4 engine wrote
//! (see that folder's README for how, and for the expected contents).

use crate::common::*;
use dissent_core::{CommunityId, ConvId, Engine, Identity, NodeConfig, RelayChoice, Status};
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
    // A v0.1.4 profile has no history keys file. Its messages still read, and none is marked locked.
    assert!(messages.iter().all(|m| !m.locked && !m.before_join), "v0.1.4 messages still read");
    let welcome = messages
        .iter()
        .find(|m| m.text == "welcome to the fixture server")
        .expect("welcome message survives");
    assert_eq!(welcome.reactions.len(), 1, "one reaction on the welcome message");
    // Messages from v0.1.4 have no mentions trailer, so none of them mentions anyone.
    assert!(messages.iter().all(|m| !m.mentions_me && m.mentions.is_empty()), "no mentions in a v0.1.4 profile");
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

const PASSPHRASE_V0111: &str = "dissent fixture v0.1.11";
const SALT_V0111: [u8; 16] = *b"dissent-fixture2";

fn fixture_v0111() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/profile-v0.1.11")
}

/// A profile written by v0.1.11, which still has a server, a channel and its messages. Each start must show
/// the same thing, and the engine's own writes must keep it readable.
#[tokio::test(flavor = "multi_thread")]
async fn v0111_profile_loads_and_views_match() {
    bounded(async {
        let work = scratch("upgrade-compat-v0111");
        copy_dir(&fixture_v0111().join("engine"), &work.join("engine"));
        fs::copy(fixture_v0111().join("identity.bin"), work.join("identity.bin")).unwrap();

        let vault = Arc::new(Vault::from_passphrase(PASSPHRASE_V0111, &SALT_V0111).unwrap());
        let identity = keystore::load(&work.join("identity.bin"), &vault)
            .expect("the v0.1.11 identity opens with the fixture passphrase");

        for run in 1..=2 {
            let engine = start(&identity, &work.join("engine"), &vault).await;
            let communities = engine.communities();
            assert_eq!(communities.len(), 1, "run {run}: one community");
            assert_eq!(communities[0].name, "Fixture Server");
            assert_eq!(communities[0].my_role, "owner");
            let id: CommunityId = communities[0].id.parse().expect("community id parses");
            let view = engine.community_view(id).expect("the community opens");
            let general = view
                .channels
                .iter()
                .find(|c| c.name == "general")
                .expect("general exists")
                .id
                .parse()
                .expect("channel id parses");
            let mut texts: Vec<String> = engine
                .channel_messages(id, general, 10)
                .into_iter()
                .map(|m| m.text)
                .collect();
            texts.sort();
            assert_eq!(
                texts,
                ["second message from alice", "welcome to the fixture server"],
                "run {run}: both messages still read"
            );
            engine
                .shutdown()
                .await
                .unwrap_or_else(|e| panic!("clean shutdown after run {run}: {e}"));
        }
        let _ = fs::remove_dir_all(&work);
    })
    .await;
}

const PASSPHRASE_V0120: &str = "dissent fixture v0.1.20";
const SALT_V0120: [u8; 16] = *b"dissent-fixture3";

fn fixture_v0120() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/profile-v0.1.20")
}

/// Text the v0.1.20 fixture holds in plaintext files. None of it may be on disk once the current engine has started.
const SECRETS: [&str; 6] = [
    "secret dm from alice",
    "secret dm from bob",
    "secret group hello",
    "secret note about bob",
    "secret channel text",
    "secret crew",
];

/// Every file under `dir`, but the blob store, whose bytes are ciphertext.
fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            if entry.file_name() != "files" {
                files_under(&entry.path(), out);
            }
        } else {
            out.push(entry.path());
        }
    }
}

fn holds_a_secret(file: &Path) -> Option<&'static str> {
    let bytes = fs::read(file).unwrap();
    SECRETS.into_iter().find(|s| bytes.windows(s.len()).any(|w| w == s.as_bytes()))
}

/// A profile written by v0.1.20, from before the store was sealed: DMs, a group, a server with a picture, an avatar,
/// a bookmark, a reminder, a note and a nickname, all in plaintext files. The first start seals every one of them,
/// and each start after shows the same contents.
#[tokio::test(flavor = "multi_thread")]
async fn v0120_profile_is_sealed_on_first_start_and_reads_back() {
    use dissent_identity::RootId;
    bounded(async {
        let work = scratch("upgrade-compat-v0120");
        copy_dir(&fixture_v0120().join("engine"), &work.join("engine"));
        fs::copy(fixture_v0120().join("identity.bin"), work.join("identity.bin")).unwrap();
        let mut before = Vec::new();
        files_under(&work.join("engine"), &mut before);
        let plain = before.iter().filter(|f| holds_a_secret(f).is_some()).count();
        assert!(plain >= 4, "the fixture holds its secrets in plaintext, in {plain} files");

        let vault = Arc::new(Vault::from_passphrase(PASSPHRASE_V0120, &SALT_V0120).unwrap());
        let identity = keystore::load(&work.join("identity.bin"), &vault).expect("the v0.1.20 identity opens");
        let bob: RootId = "dgdqn7eoqkned7muhu5zuei66h2m2oejyhfjilw3yilqqjtmdo2q".parse().unwrap();
        let carol: RootId = "2f5st7z4tck62fgmwhsdu635kgbf65soytdjrqsign4yx7x2upfq".parse().unwrap();
        let group: ConvId = "kpsqinvdx6tmfma7zqb6tayybhamc27do5ahngigr5m5tklqggqq".parse().unwrap();

        for run in 1..=3 {
            let engine = start(&identity, &work.join("engine"), &vault).await;
            let mut dm: Vec<String> = engine.dm_messages(bob, 20).into_iter().map(|m| m.text).collect();
            dm.sort();
            assert_eq!(dm, ["secret dm from alice, edited", "secret dm from bob"], "run {run}: the DMs and the edit read");
            let group_texts: Vec<String> = engine.conversation_messages(group, 10).into_iter().map(|m| m.text).collect();
            assert_eq!(group_texts, ["secret group hello"], "run {run}: the group reads");
            assert!(engine.conversations().iter().any(|c| c.kind == "group" && c.name == "secret crew"));

            let communities = engine.communities();
            assert_eq!(communities.len(), 1, "run {run}: one server");
            let id: CommunityId = communities[0].id.parse().unwrap();
            let view = engine.community_view(id).expect("the server opens");
            let general = view.channels.iter().find(|c| c.name == "general").unwrap().id.parse().unwrap();
            let messages = engine.channel_messages(id, general, 10);
            assert!(messages.iter().any(|m| m.text == "secret channel text"), "run {run}: the channel reads");
            let picture = messages.iter().find(|m| m.text == "a picture").expect("the picture message");
            let thumb = picture.attachments[0].thumb.as_ref().expect("the picture keeps its thumbnail");
            let thumb_path = thumb.path.as_ref().expect("the thumbnail is on this device");
            let (jpeg, mime) = engine.open_picture(Path::new(thumb_path)).expect("the thumbnail opens");
            assert_eq!(mime, "image/jpeg");
            assert!(image::load_from_memory(&jpeg).is_ok(), "run {run}: the thumbnail decodes");
            let avatar = engine.me().avatar.expect("alice's picture is on this device");
            let (png, _) = engine.open_picture(Path::new(&avatar)).expect("the avatar opens");
            assert!(image::load_from_memory(&png).is_ok(), "run {run}: the avatar decodes");

            let bookmarks = engine.list_bookmarks();
            assert_eq!(bookmarks.len(), 1);
            assert_eq!(bookmarks[0].message.text, "secret channel text");
            let reminders = engine.list_reminders();
            assert_eq!(reminders.len(), 1);
            assert_eq!(reminders[0].message.text, "secret dm from alice");
            assert_eq!(engine.user_profile(bob).note.as_deref(), Some("secret note about bob"));
            assert_eq!(engine.user_profile(carol).nickname.as_deref(), Some("Caz"));
            let mut contacts: Vec<String> = engine.list_contacts().contacts.into_iter().map(|c| c.display_name).collect();
            contacts.sort();
            assert_eq!(contacts, ["bob", "carol"], "run {run}: the contacts");
            engine.shutdown().await.unwrap_or_else(|e| panic!("clean shutdown after run {run}: {e}"));

            // Nothing of it is left in plaintext, and the migration's backup is gone.
            let mut after = Vec::new();
            files_under(&work.join("engine"), &mut after);
            for file in &after {
                assert_eq!(holds_a_secret(file), None, "run {run}: {} holds plaintext", file.display());
            }
            assert!(!work.join("engine").join("unsealed-backup").exists(), "run {run}: the backup is removed");
        }
        let _ = fs::remove_dir_all(&work);
    })
    .await;
}
