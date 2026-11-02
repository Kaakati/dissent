//! Custom roles and bots, as the app uses them. Replay rules live in `dissent_events::roles`. See
//! `docs/design/14-file-tracker.md`, "Phase 0".
//!
//! * A custom role's bits always count on the new kinds (roles, bots, files). On the kinds released clients know they
//!   count only once the owner turns on `CustomRolePerms`. A member device that answers `Request::Features` without
//!   [`FEATURE_CUSTOM_ROLES`] holds that back, and so does one that does not answer, until the owner confirms it by
//!   name: see [`GateStatus`].
//! * A bot is a fresh identity this device makes: its root is registered with `BotRegister`, and the bot joins
//!   through an invite. [`BotToken`] carries both to the headless app that runs it.

use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

use dissent_events::{ChannelId, Payload, Perms, RoleId, RoleSpec};
use dissent_identity::{CommunityId, DeviceId, RootId, RootKey};
use serde::Serialize;

use crate::engine::{not_found, Engine};
use crate::error::{CoreError, Result};
use crate::rpc::{Request, Response};

/// This device's replay knows custom roles and their switch.
pub const FEATURE_CUSTOM_ROLES: u64 = 1 << 0;
/// This device's replay and views know the file tracker. See `tracker`.
pub const FEATURE_TRACKER: u64 = 1 << 1;
/// Every feature this version answers `Request::Features` with.
pub const FEATURES: u64 = FEATURE_CUSTOM_ROLES | FEATURE_TRACKER;

/// How long a device's answer to `Request::Features` counts before the device is asked again.
const FEATURES_FRESH: Duration = Duration::from_secs(10 * 60);
/// How long one device has to answer `Request::Features`.
const ASK_WAIT: Duration = Duration::from_secs(5);
/// How many devices a compatibility check asks at once.
const ASK_AT_ONCE: usize = 32;

/// A member device that did not say yes to a compatibility check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GateDevice {
    pub device: String,
    pub root_id: String,
    /// The member's display name.
    pub name: String,
}

/// What a switch older apps cannot follow is waiting for. The owner decides: nothing here turns a switch on by itself,
/// and every device listed must be confirmed by id (`GateStatus::unconfirmed`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct GateStatus {
    /// Devices that answered yes.
    pub ready: usize,
    /// Devices that gave no answer: offline, or out of reach. Nobody can tell what version they run.
    pub offline: Vec<GateDevice>,
    /// Devices that answered, and are too old for the switch.
    pub outdated: Vec<GateDevice>,
}

impl GateStatus {
    /// The listed devices that are not in `confirmed`. A switch goes on only when this is empty.
    pub fn unconfirmed(&self, confirmed: &[DeviceId]) -> Vec<&GateDevice> {
        self.offline
            .iter()
            .chain(&self.outdated)
            .filter(|d| !confirmed.iter().any(|c| c.to_string() == d.device))
            .collect()
    }

    /// Refuses unless every listed device is in `confirmed`, naming those that are not.
    pub(crate) fn check(&self, confirmed: &[DeviceId]) -> Result<()> {
        let left = self.unconfirmed(confirmed);
        if left.is_empty() {
            return Ok(());
        }
        let names: Vec<&str> = left.iter().map(|d| d.name.as_str()).collect();
        Err(CoreError::Invalid(format!(
            "{} member device(s) must be confirmed first, since they may not follow this: {}",
            left.len(),
            names.join(", ")
        )))
    }
}

/// What one device said to a compatibility check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GateAnswer {
    Ready,
    Outdated,
    Silent,
}

/// Reads a device's reply to a compatibility check. A device that predates the question answers
/// `Response::Error("bad request: ...")`, as every release has, so it is online and too old. Any other refusal, a
/// timeout or a failed dial says nothing about its version.
fn gate_answer(reply: Option<Result<Response>>, yes: impl Fn(&Response) -> bool) -> GateAnswer {
    match reply {
        Some(Ok(Response::Error(e))) if e.starts_with("bad request") => GateAnswer::Outdated,
        Some(Ok(Response::Error(_))) | Some(Err(_)) | None => GateAnswer::Silent,
        Some(Ok(answer)) if yes(&answer) => GateAnswer::Ready,
        Some(Ok(_)) => GateAnswer::Outdated,
    }
}

/// The question a compatibility check asks.
#[derive(Clone, Copy, Debug)]
pub(crate) enum GateProbe {
    /// `Request::Features`, with these bits.
    Feature(u64),
    /// `Request::MemberSealedSupport`, which reaches devices from before `Request::Features`.
    MemberSealed(CommunityId),
}

/// The prefix of a bot token.
pub const BOT_TOKEN_PREFIX: &str = "dissent-bot:";

/// One custom role, as the UI shows it.
#[derive(Clone, Debug, Serialize)]
pub struct RoleView {
    pub id: String,
    pub name: String,
    /// `#rrggbb`, or empty for no colour.
    pub color: String,
    /// The names of the known bits, see `Perms::NAMED`.
    pub perms: Vec<String>,
    pub position: u16,
    /// Root ids of the members who hold it.
    pub members: Vec<String>,
}

/// A registered bot.
#[derive(Clone, Debug, Serialize)]
pub struct BotView {
    pub root_id: String,
    pub name: String,
    /// Whether the bot has joined.
    pub joined: bool,
}

/// A community's custom roles and bots.
#[derive(Clone, Debug, Serialize)]
pub struct RolesView {
    /// Highest position first.
    pub roles: Vec<RoleView>,
    pub bots: Vec<BotView>,
    /// Every known permission name, in bit order, for the editor.
    pub all_perms: Vec<String>,
    /// The bits this device's person holds.
    pub my_perms: Vec<String>,
    /// The highest position this person may give a role, exclusive. `None` for moderators and above, who may give any.
    pub my_ceiling: Option<u16>,
    /// Whether custom roles' bits count on released kinds (`CustomRolePerms`).
    pub honored: bool,
}

/// What `Engine::create_bot` hands back: the token to run the bot with. It is a secret: it holds the bot's root key.
#[derive(Clone, Debug, Serialize)]
pub struct BotToken {
    pub root_id: String,
    pub token: String,
}

/// A bot's identity and invite, parsed from a token.
pub struct ParsedBotToken {
    pub root: RootKey,
    pub invite: String,
}

impl ParsedBotToken {
    /// A fresh device for the bot, certified by its root.
    pub fn identity(&self, now_ms: u64) -> dissent_identity::Identity {
        let device = dissent_identity::DeviceKey::generate();
        let cert = self.root.certify(device.id(), now_ms);
        dissent_identity::Identity { root: Some(self.root.clone()), device, cert, device_name: "bot".into() }
    }
}

/// Parses a token made by `create_bot`.
pub fn parse_bot_token(token: &str) -> Result<ParsedBotToken> {
    let rest = token
        .trim()
        .strip_prefix(BOT_TOKEN_PREFIX)
        .ok_or_else(|| CoreError::Invalid("not a bot token".into()))?;
    let (seed, invite) = rest.split_once(':').ok_or_else(|| CoreError::Invalid("bot token has no invite".into()))?;
    let bytes = data_encoding::BASE32_NOPAD
        .decode(seed.to_ascii_uppercase().as_bytes())
        .map_err(|_| CoreError::Invalid("bad bot token".into()))?;
    let seed: [u8; 32] = bytes.try_into().map_err(|_| CoreError::Invalid("bad bot token".into()))?;
    Ok(ParsedBotToken { root: RootKey::from_seed(seed), invite: invite.to_string() })
}

fn color_text(color: u32) -> String {
    if color == 0 {
        String::new()
    } else {
        format!("#{color:06x}")
    }
}

/// Parses `#rrggbb` (or empty for none).
pub fn parse_color(text: &str) -> Result<u32> {
    let t = text.trim().trim_start_matches('#');
    if t.is_empty() {
        return Ok(0);
    }
    u32::from_str_radix(t, 16)
        .ok()
        .filter(|c| t.len() == 6 && *c <= 0xff_ffff)
        .ok_or_else(|| CoreError::Invalid("colour must be #rrggbb".into()))
}

/// The spec for the UI's fields. Unknown permission names are refused.
pub fn role_spec(name: &str, color: &str, perms: &[String], position: u16) -> Result<RoleSpec> {
    let perms = Perms::from_names(perms.iter().map(String::as_str))
        .ok_or_else(|| CoreError::Invalid("unknown permission".into()))?;
    Ok(RoleSpec { name: name.trim().to_string(), color: parse_color(color)?, perms, position })
}

impl Engine {
    /// The custom roles and bots of `community`.
    pub fn roles_view(&self, community: CommunityId) -> Result<RolesView> {
        let w = self.read();
        let c = w.communities.get(&community).ok_or_else(|| not_found(&community))?;
        let st = c.state();
        let mut roles: Vec<RoleView> = st
            .custom
            .roles
            .iter()
            .map(|(id, (_, spec))| RoleView {
                id: id.to_string(),
                name: spec.name.clone(),
                color: color_text(spec.color),
                perms: spec.perms.names().into_iter().map(String::from).collect(),
                position: spec.position,
                members: st
                    .custom
                    .assigned
                    .iter()
                    .filter(|(root, _)| st.custom.roles_of(root).contains(id))
                    .map(|(root, _)| root.to_string())
                    .collect(),
            })
            .collect();
        roles.sort_by(|a, b| b.position.cmp(&a.position).then_with(|| a.name.cmp(&b.name)));
        let bots = st
            .custom
            .bots
            .iter()
            .filter(|(_, b)| b.on)
            .map(|(root, b)| BotView {
                root_id: root.to_string(),
                name: b.name.clone(),
                joined: c.is_member(root),
            })
            .collect();
        let me = w.me;
        let rank = c.role_of(&me);
        let my_ceiling = match rank {
            "owner" | "admin" | "moderator" => None,
            _ => Some(
                st.custom
                    .roles_of(&me)
                    .iter()
                    .filter_map(|r| st.custom.roles.get(r))
                    .map(|(_, s)| s.position)
                    .max()
                    .unwrap_or(0),
            ),
        };
        Ok(RolesView {
            roles,
            bots,
            all_perms: Perms::NAMED.iter().map(|(_, n)| n.to_string()).collect(),
            my_perms: st.perms_of(&me).names().into_iter().map(String::from).collect(),
            my_ceiling,
            honored: st.custom.honored,
        })
    }

    /// The custom roles `user` holds in `community`, as role ids, and whether it is a bot.
    pub fn member_roles(&self, community: CommunityId, user: RootId) -> Result<(Vec<String>, bool)> {
        let w = self.read();
        let c = w.communities.get(&community).ok_or_else(|| not_found(&community))?;
        let st = c.state();
        let root = st.canonical(&user);
        Ok((st.custom.roles_of(&root).iter().map(RoleId::to_string).collect(), st.is_bot(&root)))
    }

    /// Makes a custom role and returns its id.
    pub async fn create_role(&self, community: CommunityId, spec: RoleSpec) -> Result<RoleId> {
        let se = self.author_publish(community, ChannelId::CONTROL, Payload::RoleCreate { spec }).await?;
        Ok(RoleId(se.event.id.0))
    }

    pub async fn update_role(&self, community: CommunityId, role: RoleId, spec: RoleSpec) -> Result<()> {
        self.author_publish(community, ChannelId::CONTROL, Payload::RoleUpdate { role, spec }).await?;
        Ok(())
    }

    pub async fn delete_role(&self, community: CommunityId, role: RoleId) -> Result<()> {
        self.author_publish(community, ChannelId::CONTROL, Payload::RoleDelete { role }).await?;
        Ok(())
    }

    /// Gives `user` the role, or takes it away when `on` is false.
    pub async fn assign_role(&self, community: CommunityId, user: RootId, role: RoleId, on: bool) -> Result<()> {
        self.author_publish(community, ChannelId::CONTROL, Payload::RoleAssign { user, role, on }).await?;
        Ok(())
    }

    /// Marks `root` as a bot named `name`, or unmarks it.
    pub async fn register_bot(&self, community: CommunityId, root: RootId, name: String, on: bool) -> Result<()> {
        let name = name.trim().to_string();
        self.author_publish(community, ChannelId::CONTROL, Payload::BotRegister { root, name, on }).await?;
        Ok(())
    }

    /// Makes a bot for `community`: a fresh root, its registration, an invite for it, and the roles in `roles`.
    /// Returns the token the bot runs with. Nothing of the root key stays on this device.
    pub async fn create_bot(&self, community: CommunityId, name: String, roles: Vec<RoleId>) -> Result<BotToken> {
        let root = RootKey::generate();
        let id = root.id();
        self.register_bot(community, id, name, true).await?;
        for role in roles {
            self.assign_role(community, id, role, true).await?;
        }
        let invite = self.create_invite(community).await?;
        let seed = data_encoding::BASE32_NOPAD.encode(&root.0.seed()).to_lowercase();
        Ok(BotToken { root_id: id.to_string(), token: format!("{BOT_TOKEN_PREFIX}{seed}:{invite}") })
    }

    /// Turns on or off whether custom roles' bits count on the kinds released clients know. Owner only. Turning it on
    /// is refused while `custom_role_perms_status` lists a device: see `set_custom_role_perms_confirmed`.
    pub async fn set_custom_role_perms(&self, community: CommunityId, on: bool) -> Result<()> {
        self.set_custom_role_perms_confirmed(community, on, &[]).await
    }

    /// As `set_custom_role_perms`, going ahead for the listed devices in `confirmed`: the owner was told what those
    /// people see until they update. A device that newly turned up is still refused.
    pub async fn set_custom_role_perms_confirmed(&self, community: CommunityId, on: bool, confirmed: &[DeviceId]) -> Result<()> {
        if on {
            self.custom_role_perms_status(community).await.check(confirmed)?;
        }
        self.author_publish(community, ChannelId::CONTROL, Payload::CustomRolePerms { on }).await?;
        Ok(())
    }

    /// Which member devices could not follow `CustomRolePerms`.
    pub async fn custom_role_perms_status(&self, community: CommunityId) -> GateStatus {
        self.gate_status(community, GateProbe::Feature(FEATURE_CUSTOM_ROLES)).await
    }

    /// Asks every device of a current member, at once, and sorts them by what they said.
    pub(crate) async fn gate_status(&self, community: CommunityId, probe: GateProbe) -> GateStatus {
        let limit = Arc::new(tokio::sync::Semaphore::new(ASK_AT_ONCE));
        let mut asks = tokio::task::JoinSet::new();
        for (device, root, name) in self.member_devices(community) {
            let engine = self.clone();
            let limit = limit.clone();
            asks.spawn(async move {
                let _turn = limit.acquire_owned().await;
                (GateDevice { device: device.to_string(), root_id: root.to_string(), name }, engine.ask_gate(device, probe).await)
            });
        }
        let mut status = GateStatus::default();
        while let Some(done) = asks.join_next().await {
            let Ok((device, answer)) = done else { continue };
            match answer {
                GateAnswer::Ready => status.ready += 1,
                GateAnswer::Outdated => status.outdated.push(device),
                GateAnswer::Silent => status.offline.push(device),
            }
        }
        status.offline.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.device.cmp(&b.device)));
        status.outdated.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.device.cmp(&b.device)));
        status
    }

    async fn ask_gate(&self, device: DeviceId, probe: GateProbe) -> GateAnswer {
        match probe {
            GateProbe::Feature(feature) => {
                let has = |bits: u64| bits & feature == feature;
                // Only a yes is kept: a device that was offline or old is asked again next time.
                let known = {
                    let answers = self.inner.feature_answers.lock().unwrap_or_else(PoisonError::into_inner);
                    answers.get(&device).is_some_and(|(bits, at)| at.elapsed() < FEATURES_FRESH && has(*bits))
                };
                if known {
                    return GateAnswer::Ready;
                }
                let reply = tokio::time::timeout(ASK_WAIT, self.call(device, &Request::Features)).await.ok();
                if let Some(Ok(Response::Features(bits))) = &reply {
                    if has(*bits) {
                        self.inner
                            .feature_answers
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .insert(device, (*bits, Instant::now()));
                    }
                }
                gate_answer(reply, |r| matches!(r, Response::Features(bits) if has(*bits)))
            }
            GateProbe::MemberSealed(community) => {
                let reply = tokio::time::timeout(ASK_WAIT, self.call(device, &Request::MemberSealedSupport { community })).await.ok();
                gate_answer(reply, |r| matches!(r, Response::MemberSealedSupport(true)))
            }
        }
    }

    /// Every device of a current member with its root and name, this device's siblings and storage members included,
    /// but not this device.
    fn member_devices(&self, community: CommunityId) -> Vec<(DeviceId, RootId, String)> {
        let w = self.read();
        let Some(c) = w.communities.get(&community) else {
            return Vec::new();
        };
        let st = c.state();
        st.members
            .iter()
            .filter(|(root, m)| !m.left && !st.banned.contains(root))
            .flat_map(|(root, _)| {
                let name = w.display_name(root);
                w.directory.devices_of(root).into_iter().map(move |d| (d, *root, name.clone()))
            })
            .filter(|(d, ..)| *d != w.device && !st.revoked_devices.contains_key(d))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate_device(name: &str, device: DeviceId) -> GateDevice {
        GateDevice { device: device.to_string(), root_id: String::new(), name: name.into() }
    }

    #[test]
    fn a_gate_tells_an_old_device_from_a_silent_one() {
        let yes = |r: &Response| matches!(r, Response::Features(b) if b & FEATURE_CUSTOM_ROLES != 0);
        let old = Some(Ok(Response::Error("bad request: Hit the end of buffer".into())));
        assert_eq!(gate_answer(old, yes), GateAnswer::Outdated, "a release from before the question");
        assert_eq!(gate_answer(Some(Ok(Response::Features(FEATURE_TRACKER))), yes), GateAnswer::Outdated, "lacks the bit");
        assert_eq!(gate_answer(Some(Ok(Response::Features(FEATURES))), yes), GateAnswer::Ready);
        assert_eq!(gate_answer(Some(Ok(Response::Error("rate limited".into()))), yes), GateAnswer::Silent);
        assert_eq!(gate_answer(Some(Err(CoreError::Invalid("dial".into()))), yes), GateAnswer::Silent);
        assert_eq!(gate_answer(None, yes), GateAnswer::Silent, "no answer in time");
    }

    #[test]
    fn a_switch_waits_until_every_listed_device_is_confirmed() {
        let (carol, bob) = (dissent_identity::DeviceKey::generate().id(), dissent_identity::DeviceKey::generate().id());
        let status = GateStatus { ready: 1, offline: vec![gate_device("Carol", carol)], outdated: vec![gate_device("Bob", bob)] };
        let refused = status.check(&[]).unwrap_err().to_string();
        assert!(refused.contains("2 member device(s)") && refused.contains("Carol") && refused.contains("Bob"), "{refused}");
        let refused = status.check(&[carol]).unwrap_err().to_string();
        assert!(refused.contains("Bob") && !refused.contains("Carol"), "{refused}");
        status.check(&[carol, bob]).expect("every listed device is confirmed");
        GateStatus::default().check(&[]).expect("nobody to wait for");
    }

    #[test]
    fn colours_parse_and_print() {
        assert_eq!(parse_color("#3366ff").unwrap(), 0x3366ff);
        assert_eq!(parse_color("").unwrap(), 0);
        assert!(parse_color("#12345").is_err());
        assert!(parse_color("#zzzzzz").is_err());
        assert_eq!(color_text(0x3366ff), "#3366ff");
        assert_eq!(color_text(0), "");
    }

    #[test]
    fn a_bot_token_round_trips() {
        let root = RootKey::generate();
        let seed = data_encoding::BASE32_NOPAD.encode(&root.0.seed()).to_lowercase();
        let token = format!("{BOT_TOKEN_PREFIX}{seed}:dissent://invite/abc");
        let parsed = parse_bot_token(&token).unwrap();
        assert_eq!(parsed.root.id(), root.id());
        assert_eq!(parsed.invite, "dissent://invite/abc");
        assert!(parse_bot_token("nope").is_err());
    }

    #[test]
    fn unknown_permission_names_are_refused() {
        assert!(role_spec("Testers", "", &["manage_files".into()], 1).is_ok());
        assert!(role_spec("Testers", "", &["fly".into()], 1).is_err());
    }
}
