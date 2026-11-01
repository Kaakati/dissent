//! Custom roles and bots: an admin makes a role and gives it to a member, its bits count on released kinds only
//! once the owner turns that on, the switch waits for every member device, and a bot made from a token joins and
//! shows as a bot. See `docs/design/14-file-tracker.md`, "Phase 0". The networked tests are `#[ignore]`d and run
//! through `scripts/fwtest.sh`.

use std::time::Duration;

use crate::common::*;
use dissent_core::custom_roles::{parse_bot_token, role_spec};
use dissent_core::CommunityId;
use dissent_events::RoleId;

#[tokio::test(flavor = "multi_thread")]
async fn an_owner_alone_makes_edits_and_deletes_a_role() {
    bounded(async {
        let alice = peer("roles-alone").await;
        let server = alice.engine.create_community("Makers".into()).await.expect("create");
        let id: CommunityId = server.id.parse().unwrap();
        let spec = role_spec("Testers", "#3366ff", &["manage_files".into()], 2).unwrap();
        let role = alice.engine.create_role(id, spec).await.expect("create role");
        let me = alice.identity.root_id();
        alice.engine.assign_role(id, me, role, true).await.expect("assign");

        let view = alice.engine.roles_view(id).unwrap();
        assert_eq!(view.roles.len(), 1);
        assert_eq!(view.roles[0].name, "Testers");
        assert_eq!(view.roles[0].color, "#3366ff");
        assert_eq!(view.roles[0].perms, vec!["manage_files"]);
        assert_eq!(view.roles[0].members, vec![me.to_string()]);
        let member = alice.engine.community_view(id).unwrap().members.into_iter().find(|m| m.root_id == me.to_string());
        assert_eq!(member.unwrap().roles, vec![role.to_string()]);

        let renamed = role_spec("QA", "", &[], 2).unwrap();
        alice.engine.update_role(id, role, renamed).await.expect("update");
        assert_eq!(alice.engine.roles_view(id).unwrap().roles[0].name, "QA");
        assert!(alice.engine.create_role(id, role_spec("", "", &[], 1).unwrap()).await.is_err(), "a blank name");

        // With no other member device, the switch has nobody to wait for.
        alice.engine.set_custom_role_perms(id, true).await.expect("switch on");
        assert!(alice.engine.roles_view(id).unwrap().honored);

        alice.engine.delete_role(id, role).await.expect("delete");
        assert!(alice.engine.roles_view(id).unwrap().roles.is_empty());
        finish(vec![alice]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_role_grants_bits_once_every_device_knows_custom_roles() {
    heavy(2, async {
        let alice = peer("roles-alice").await;
        let bob = peer("roles-bob").await;
        let id = rustaceans(&alice, &bob).await;
        let bob_root = bob.identity.root_id();

        let spec = role_spec("Builders", "", &["manage_channels".into()], 1).unwrap();
        let role: RoleId = alice.engine.create_role(id, spec).await.expect("create role");
        alice.engine.assign_role(id, bob_root, role, true).await.expect("assign");
        until_event(&bob.engine, Duration::from_secs(60), "bob sees his role", || {
            bob.engine
                .member_roles(id, bob_root)
                .ok()
                .filter(|(roles, _)| roles == &vec![role.to_string()])
        })
        .await;
        assert!(bob.engine.roles_view(id).unwrap().my_perms.contains(&"manage_channels".to_string()));
        assert!(bob.engine.create_channel(id, "early".into()).await.is_err(), "released kinds wait for the switch");

        // Bob's device stands in for a release before custom roles: the switch is refused.
        dissent_core::test_support::set_features(&bob.engine, 0);
        let refused = alice.engine.set_custom_role_perms(id, true).await;
        assert!(refused.is_err(), "{refused:?}");
        let status = alice.engine.custom_role_perms_status(id).await;
        assert_eq!((status.outdated.len(), status.offline.len()), (1, 0), "an online device that answers is not offline");
        dissent_core::test_support::set_features(&bob.engine, dissent_core::custom_roles::FEATURES);
        alice.engine.set_custom_role_perms(id, true).await.expect("switch on");

        until_event(&bob.engine, Duration::from_secs(60), "bob sees the switch", || {
            bob.engine.roles_view(id).ok().filter(|v| v.honored).map(|_| ())
        })
        .await;
        bob.engine.create_channel(id, "builds".into()).await.expect("bob makes a channel");
        until_event(&alice.engine, Duration::from_secs(60), "alice accepts bob's channel", || {
            channel_id(&alice.engine, id, "builds")
        })
        .await;
        finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn the_owner_overrides_an_old_or_offline_device_only_by_naming_it() {
    heavy(2, async {
        let alice = peer("gate-alice").await;
        let bob = peer("gate-bob").await;
        let id = rustaceans(&alice, &bob).await;
        let bob_device = bob.identity.device_id();

        // Online and too old: blocks until the owner names it.
        dissent_core::test_support::set_features(&bob.engine, 0);
        let status = alice.engine.custom_role_perms_status(id).await;
        assert_eq!(status.outdated.len(), 1, "{status:?}");
        assert_eq!(status.outdated[0].device, bob_device.to_string());
        assert!(alice.engine.set_custom_role_perms(id, true).await.is_err(), "never silently");
        let member_only = alice.engine.member_only_status(id).await.unwrap();
        assert!(!member_only.can_enable && member_only.outdated.len() == 1, "{member_only:?}");
        alice.engine.set_custom_role_perms_confirmed(id, true, &[bob_device]).await.expect("the owner overrides");
        assert!(alice.engine.roles_view(id).unwrap().honored);
        alice.engine.set_custom_role_perms(id, false).await.expect("turning off asks nothing");
        dissent_core::test_support::set_features(&bob.engine, dissent_core::custom_roles::FEATURES);

        // Offline: warned about by name, still refused until confirmed, and an unrelated confirmation does not help.
        let (bob_identity, bob_dir) = stop(bob).await;
        let status = alice.engine.custom_role_perms_status(id).await;
        assert_eq!((status.offline.len(), status.outdated.len()), (1, 0), "{status:?}");
        assert_eq!(status.offline[0].name, "gate-bob");
        let stranger = dissent_identity::DeviceKey::generate().id();
        let refused = alice.engine.set_custom_role_perms_confirmed(id, true, &[stranger]).await;
        assert!(refused.is_err_and(|e| e.to_string().contains("gate-bob")));
        alice.engine.set_custom_role_perms_confirmed(id, true, &[bob_device]).await.expect("confirmed by name");

        // The member comes back and catches up.
        let bob = start(bob_identity, bob_dir).await;
        until_event(&bob.engine, Duration::from_secs(60), "bob catches up on the switch", || {
            bob.engine.roles_view(id).ok().filter(|v| v.honored).map(|_| ())
        })
        .await;
        finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_bot_made_from_a_token_joins_with_its_role() {
    heavy(2, async {
        let alice = peer("bot-owner").await;
        let server = alice.engine.create_community("Builds".into()).await.expect("create");
        let id: CommunityId = server.id.parse().unwrap();
        let role = alice.engine.create_role(id, role_spec("CI", "", &[], 1).unwrap()).await.unwrap();
        let token = alice.engine.create_bot(id, "CI bot".into(), vec![role]).await.expect("bot");
        let parsed = parse_bot_token(&token.token).expect("token parses");
        assert_eq!(parsed.root.id().to_string(), token.root_id);

        let bot = start(parsed.identity(now()), scratch("bot-runner")).await;
        join_ready(&alice, &bot, &parsed.invite).await;
        let view = alice.engine.community_view(id).unwrap();
        let member = view.members.iter().find(|m| m.root_id == token.root_id).expect("the bot is listed");
        assert!(member.bot);
        assert_eq!(member.roles, vec![role.to_string()]);
        let bots = alice.engine.roles_view(id).unwrap().bots;
        assert_eq!(bots.len(), 1);
        assert!(bots[0].joined);
        finish(vec![alice, bot]).await;
    })
    .await;
}
