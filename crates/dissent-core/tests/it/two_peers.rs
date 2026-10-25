//! Networked end-to-end tests. Every engine runs on this machine with relays disabled.

use dissent_core::{
    ChannelId, CommunityId, CoreError, Engine, EngineEvent, Identity, ModAction, NodeConfig,
    RelayChoice, RootId,
};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

/// Longest any one test may run. A hang fails here instead of stalling the whole run.
const TEST_LIMIT: Duration = Duration::from_secs(240);

async fn bounded(body: impl std::future::Future<Output = ()>) {
    tokio::time::timeout(TEST_LIMIT, body)
        .await
        .unwrap_or_else(|_| panic!("test exceeded {TEST_LIMIT:?}"));
}

struct Peer {
    engine: Engine,
    identity: Identity,
    dir: PathBuf,
    /// Kept so the notification channel stays open while the test runs.
    _events: broadcast::Receiver<EngineEvent>,
}

fn scratch(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    // Tests run in parallel. A counter keeps two calls from sharing one directory.
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "dissent-core-it-{name}-{}-{nanos}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn offline() -> NodeConfig {
    NodeConfig {
        relay: RelayChoice::Disabled,
        loopback_only: true,
        ..NodeConfig::default()
    }
}

async fn start(identity: Identity, dir: PathBuf) -> Peer {
    let (engine, events) = Engine::start(identity.clone(), dir.clone(), offline(), dissent_core::test_support::vault_for(&dir))
        .await
        .expect("engine starts");
    Peer {
        engine,
        identity,
        dir,
        _events: events,
    }
}

async fn peer(name: &str) -> Peer {
    let p = start(Identity::create(now()), scratch(name)).await;
    p.engine
        .set_display_name(name.to_string())
        .await
        .expect("name set");
    p
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Polls `check` every 50 ms for up to 10 s. Panics with `what` on timeout.
async fn eventually<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Alice creates "Rustaceans" and Bob joins it. Returns the community id once Alice lists Bob, so
/// Bob's messages are admitted from then on.
async fn rustaceans(alice: &Peer, bob: &Peer) -> CommunityId {
    let server = alice
        .engine
        .create_community("Rustaceans".into())
        .await
        .expect("create community");
    let id: CommunityId = server.id.parse().unwrap();
    let invite = alice
        .engine
        .create_invite(id)
        .await
        .expect("create invite");
    let joined = bob
        .engine
        .join_community(&invite)
        .await
        .expect("join community");
    assert_eq!(joined.id, server.id);
    assert_eq!(joined.my_role, "member");
    let bob_root = bob.identity.root_id().to_string();
    eventually("alice lists bob as a member", || {
        alice
            .engine
            .community_view(id)
            .ok()
            .filter(|v| v.members.iter().any(|m| m.root_id == bob_root))
            .map(|_| ())
    })
    .await;
    id
}

fn has_message(engine: &Engine, community: CommunityId, channel: ChannelId, text: &str) -> bool {
    engine
        .channel_messages(community, channel, 100)
        .iter()
        .any(|m| m.text == text)
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


/// Shuts every engine down and deletes its data directory.
async fn finish(peers: Vec<Peer>) {
    for p in peers {
        p.engine.shutdown().await.expect("clean shutdown");
        let _ = std::fs::remove_dir_all(&p.dir);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn servers_channels_messages_and_bans() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let id = rustaceans(&alice, &bob).await;

    let view = bob.engine.community_view(id).unwrap();
    assert!(view.channels.iter().any(|c| c.name == "general"));
    assert_eq!(view.members.len(), 2, "both members listed: {:?}", view.members);
    let general = channel_id(&bob.engine, id, "general").expect("general channel");

    let hi = bob
        .engine
        .send_message(id, general, "hi".into())
        .await
        .expect("bob posts");
    assert_eq!(hi.author_name, "bob");
    eventually("alice receives hi", || {
        has_message(&alice.engine, id, general, "hi").then_some(())
    })
    .await;

    alice
        .engine
        .create_channel(id, "random".into())
        .await
        .expect("create channel");
    eventually("bob sees the random channel", || {
        channel_id(&bob.engine, id, "random").map(|_| ())
    })
    .await;

    let bob_root: RootId = bob.identity.root_id();
    alice
        .engine
        .moderate(id, ModAction::Ban, bob_root)
        .await
        .expect("alice bans bob");
    let bob_key = bob_root.to_string();
    eventually("bob learns of his ban", || {
        bob.engine
            .community_view(id)
            .ok()?
            .members
            .iter()
            .find(|m| m.root_id == bob_key)
            .filter(|m| m.banned)
            .map(|_| ())
    })
    .await;

    let err = bob
        .engine
        .send_message(id, general, "after the ban".into())
        .await
        .expect_err("banned posts are refused locally");
    assert!(matches!(err, CoreError::Rejected(_)), "{err:?}");

    // The refused message is never broadcast, so it must not appear for Alice.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!has_message(&alice.engine, id, general, "after the ban"));

    finish(vec![alice, bob]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn multiple_servers_and_mutual_profile() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    rustaceans(&alice, &bob).await;

    let gophers = bob
        .engine
        .create_community("Gophers".into())
        .await
        .expect("bob creates Gophers");
    let gophers_id: CommunityId = gophers.id.parse().unwrap();
    let invite = bob.engine.create_invite(gophers_id).await.unwrap();
    alice
        .engine
        .join_community(&invite)
        .await
        .expect("alice joins Gophers");

    assert_eq!(alice.engine.communities().len(), 2);
    assert_eq!(bob.engine.communities().len(), 2);

    let alice_root = alice.identity.root_id();
    let profile = eventually("two mutual communities", || {
        let p = bob.engine.user_profile(alice_root);
        (p.mutual_communities.len() == 2).then_some(p)
    })
    .await;
    assert_eq!(profile.display_name, "alice");
    assert_eq!(profile.handles.len(), 2);

    finish(vec![alice, bob]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn direct_messages_and_contacts() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    rustaceans(&alice, &bob).await;
    let alice_root = alice.identity.root_id();
    let bob_root = bob.identity.root_id();

    alice
        .engine
        .send_dm(bob_root, "secret".into())
        .await
        .expect("alice sends");

    eventually("bob sees alice in conversations", || {
        bob.engine
            .conversations()
            .iter()
            .find(|c| c.peer == alice_root.to_string())
            .filter(|c| c.last_text == "secret")
            .map(|_| ())
    })
    .await;

    bob.engine
        .send_dm(alice_root, "pong".into())
        .await
        .expect("bob replies");
    eventually("alice receives the reply", || {
        alice
            .engine
            .dm_messages(bob_root, 10)
            .iter()
            .any(|m| m.text == "pong")
            .then_some(())
    })
    .await;

    // Carol shares no community with Bob. She can only reach him through his contact link.
    let carol = peer("carol").await;
    let carol_root = carol.identity.root_id();
    // A link sends a request. Bob must accept before either side has a contact.
    let asked = carol
        .engine
        .add_contact(&bob.engine.contact_link())
        .await
        .expect("carol asks bob");
    assert!(!asked.is_contact, "a request is not a contact yet");
    eventually("bob sees carol's request", || {
        bob.engine
            .contact_requests()
            .iter()
            .any(|r| r.user == carol_root.to_string())
            .then_some(())
    })
    .await;
    let accepted = bob
        .engine
        .accept_request(carol_root)
        .await
        .expect("bob accepts carol");
    assert!(accepted.is_contact);
    let bob_for_carol = eventually("carol becomes bob's contact", || {
        let view = carol.engine.user_profile(bob.identity.root_id());
        view.is_contact.then_some(view)
    })
    .await;
    assert_eq!(bob_for_carol.display_name, "bob");

    carol
        .engine
        .send_dm(bob_root, "hello from carol".into())
        .await
        .expect("carol sends");
    eventually("bob receives carol", || {
        bob.engine
            .dm_messages(carol_root, 10)
            .iter()
            .any(|m| m.text == "hello from carol")
            .then_some(())
    })
    .await;

    // Bob learned Carol's address from her message, so he can answer her.
    bob.engine
        .send_dm(carol_root, "hi carol".into())
        .await
        .expect("bob answers carol");
    eventually("carol receives bob", || {
        carol
            .engine
            .dm_messages(bob_root, 10)
            .iter()
            .any(|m| m.text == "hi carol")
            .then_some(())
    })
    .await;

    finish(vec![alice, bob, carol]).await;
    }).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn data_survives_restart() {
    bounded(async {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let id = rustaceans(&alice, &bob).await;
    let general = channel_id(&alice.engine, id, "general").expect("general channel");
    let bob_root = bob.identity.root_id();

    alice
        .engine
        .send_message(id, general, "remember me".into())
        .await
        .expect("alice posts");
    alice
        .engine
        .send_dm(bob_root, "keep this".into())
        .await
        .expect("alice DMs bob");

    let Peer {
        engine,
        identity,
        dir,
        ..
    } = alice;
    engine.shutdown().await.expect("clean shutdown");

    let (reopened, _events) = Engine::start(identity.clone(), dir.clone(), offline(), dissent_core::test_support::vault_for(&dir))
        .await
        .expect("restart");
    assert_eq!(reopened.me().display_name, "alice");

    let communities = reopened.communities();
    assert_eq!(communities.len(), 1);
    assert_eq!(communities[0].name, "Rustaceans");
    assert_eq!(communities[0].id, id.to_string());
    assert!(has_message(&reopened, id, general, "remember me"));

    let conversations = reopened.conversations();
    let with_bob = conversations
        .iter()
        .find(|c| c.peer == bob_root.to_string())
        .expect("conversation with bob");
    assert_eq!(with_bob.last_text, "keep this");

    reopened.shutdown().await.expect("shutdown");
    let _ = std::fs::remove_dir_all(&dir);
    finish(vec![bob]).await;
    }).await;
}
