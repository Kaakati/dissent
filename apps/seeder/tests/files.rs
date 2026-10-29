//! Files a sender lets the seeder keep stay downloadable while the sender is offline. The seeder takes its file policy
//! from its own flags, finds the files through each message's cleartext manifest, and never gets a private channel's
//! files. Every engine runs on this machine with relays disabled.

use clap::Parser;
use dissent_core::{ChannelId, CommunityId, Engine, EngineEvent, Identity, NodeConfig, RelayChoice};
use dissent_seeder::Cli;
use std::future::Future;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

const STEP: Duration = Duration::from_secs(30);
const TEST_LIMIT: Duration = Duration::from_secs(240);

struct Node {
    engine: Engine,
    identity: Identity,
    dir: PathBuf,
    _events: broadcast::Receiver<EngineEvent>,
}

fn scratch(name: &str) -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("dissent-seeder-files-{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

async fn step<T>(what: &str, fut: impl Future<Output = T>) -> T {
    tokio::time::timeout(STEP, fut).await.unwrap_or_else(|_| panic!("{what} did not finish within {STEP:?}"))
}

async fn node(name: &str) -> Node {
    let dir = scratch(name);
    let config = NodeConfig { relay: RelayChoice::Disabled, loopback_only: true, dht_lookup: false, ..NodeConfig::default() };
    let identity = Identity::create(now());
    let (engine, events) = step(
        "engine start",
        Engine::start_with_timings(
            identity.clone(),
            dir.clone(),
            config,
            dissent_core::test_support::vault_for(&dir),
            dissent_core::test_support::fast_timings(),
        ),
    )
    .await
    .expect("engine starts");
    step("set name", engine.set_display_name(name.to_string())).await.expect("name set");
    Node { engine, identity, dir, _events: events }
}

async fn eventually<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn channel(engine: &Engine, id: CommunityId, name: &str) -> Option<ChannelId> {
    engine.community_view(id).ok()?.channels.iter().find(|c| c.name == name).and_then(|c| c.id.parse().ok())
}

async fn shut_down(node: Node) {
    step("shutdown", node.engine.shutdown()).await.expect("clean shutdown");
    let _ = std::fs::remove_dir_all(&node.dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_downloads_an_allowed_file_from_the_seeder_while_the_sender_is_offline() {
    tokio::time::timeout(TEST_LIMIT, async {
        let alice = node("alice").await;
        let server = step("create", alice.engine.create_community("Rustaceans".into())).await.expect("create");
        let id: CommunityId = server.id.parse().unwrap();
        let general = channel(&alice.engine, id, "general").expect("general");
        let invite = step("invite", alice.engine.create_invite(id)).await.expect("invite");

        // The seeder runs with its default flags, as an operator starts it.
        let seeder = node("seeder").await;
        let run = Cli::try_parse_from(["dissent-seeder", "--data-dir", "unused"]).expect("flags parse").run;
        seeder.engine.set_mirror_policy(run.mirror_policy()).expect("file policy");
        step("seeder joins", seeder.engine.join_as_storage(&invite)).await.expect("seeder joins as storage");

        let bob = node("bob").await;
        step("bob joins", bob.engine.join_community(&invite)).await.expect("bob joins");
        let bob_root = bob.identity.root_id();
        eventually("alice lists bob", || {
            alice.engine.community_view(id).ok()?.members.iter().any(|m| m.root_id == bob_root.to_string()).then_some(())
        })
        .await;

        // One file the seeder may keep, and one in a private channel, which it never gets.
        let dir = scratch("files");
        std::fs::write(dir.join("map.txt"), b"the trail map").unwrap();
        std::fs::write(dir.join("vault.txt"), b"for readers only").unwrap();
        let vault = step("private channel", alice.engine.create_private_channel(id, "vault".into(), vec![], vec![bob_root]))
            .await
            .expect("vault");
        let vault: ChannelId = vault.id.parse().unwrap();
        let shared = step("post", alice.engine.send_files(id, general, vec![dir.join("map.txt")], "map".into(), true))
            .await
            .expect("alice posts the map");
        let private = step("post private", alice.engine.send_files(id, vault, vec![dir.join("vault.txt")], "vault".into(), true))
            .await
            .expect("alice posts in vault");
        let shared_id = shared.attachments[0].id.clone();
        let private_id = private.attachments[0].id.clone();

        eventually("the seeder keeps the allowed file", || {
            seeder.engine.mirrored_attachments().contains(&shared_id).then_some(())
        })
        .await;
        eventually("bob sees the file", || {
            bob.engine.channel_messages(id, general, 20).iter().any(|m| m.id == shared.id && m.attachments.len() == 1).then_some(())
        })
        .await;

        // Alice goes offline; Bob downloads the map from the seeder alone.
        shut_down(alice).await;
        step("download", bob.engine.download_attachment(Some(id), shared.id.clone(), shared_id.clone()))
            .await
            .expect("bob starts the download");
        let local = eventually("bob has the map", || {
            bob.engine
                .channel_messages(id, general, 20)
                .into_iter()
                .find(|m| m.id == shared.id)
                .and_then(|m| m.attachments.first().and_then(|a| a.local_path.clone()))
        })
        .await;
        assert_eq!(std::fs::read(local).unwrap(), b"the trail map");
        assert!(!seeder.engine.mirrored_attachments().contains(&private_id), "a private channel's file never reaches the seeder");

        shut_down(bob).await;
        shut_down(seeder).await;
    })
    .await
    .unwrap_or_else(|_| panic!("test exceeded {TEST_LIMIT:?}"));
}

#[test]
fn the_file_flags_set_the_policy_and_no_files_turns_it_off() {
    let parse = |args: &[&str]| Cli::try_parse_from([&["dissent-seeder", "--data-dir", "d"], args].concat()).expect("parse").run;
    let default = parse(&[]).mirror_policy().expect("files are kept by default");
    assert_eq!((default.max_file_size, default.max_total), (2048 << 20, 50 << 30));
    let set = parse(&["--keep-files-up-to", "10", "--files-quota", "2"]).mirror_policy().expect("policy");
    assert_eq!((set.max_file_size, set.max_total), (10 << 20, 2 << 30));
    assert!(parse(&["--no-files"]).mirror_policy().is_none());
}
