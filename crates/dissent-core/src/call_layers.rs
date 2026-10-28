//! Simulcast signalling for forwarded calls: what each side tells the others so the forwarder can
//! send every viewer one layer of each stream. See `docs/design/13-forwarded-calls.md`, phase 2.
//!
//! - A sender tells its forwarder the layers it can send of its camera and screen (`VideoLayers`).
//! - A viewer tells its forwarder how wide it draws each sender's video (`TileWidths`). That also
//!   marks the viewer as knowing layers; a viewer that never says so gets layer 0 only.
//! - The forwarder tells each sender which layers someone is sent or waits for (`WantedLayers`).
//!   Only then does the sender switch from its single stream to the layers asked for, so a released
//!   forwarder, which never asks, keeps getting one stream.
//!
//! Every message goes only to a device that announced `FEATURE_VIDEO_LAYERS`, since a released one
//! cannot decode them.

use std::collections::BTreeMap;
use std::sync::Arc;

use dissent_identity::DeviceId;
use dissent_media::LayerInfo;
use tracing::debug;

use crate::calls::{CallMsg, CallReply, Role, Room, RoomId};
use crate::engine::Engine;

/// Tile widths as a viewer reports them: (sender id, screen, width in pixels).
pub(crate) type Tiles = Vec<(u64, bool, u32)>;
/// A layer table as a sender announces it: (width in pixels, kbps), smallest first.
pub(crate) type LayerTable = Vec<(u32, u32)>;

/// What this device last told, or was told, about layers in one room. Kept so each message goes
/// out only when it changes.
#[derive(Debug, Default)]
pub(crate) struct LayerSignals {
    /// The tile widths last sent to the forwarder, as (sender id, screen, width).
    pub told_tiles: Option<(u64, Tiles)>,
    /// The layer tables last sent to the forwarder, camera then screen, with the forwarder told.
    pub told_layers: [Option<(u64, LayerTable)>; 2],
    /// On a forwarder: the masks last sent to each sender, by (sender id, screen).
    pub told_wanted: BTreeMap<(u64, bool), u8>,
    /// The masks this device's forwarder asked for, camera then screen. `None` until asked, which
    /// means one stream as before.
    pub wanted_here: [Option<u8>; 2],
    /// Whether a round of `signal_layers` is still running, so rounds never pile up.
    pub busy: bool,
    /// The layer of the last frame received of each (sender id, screen) stream.
    pub received: BTreeMap<(u64, bool), u8>,
}

impl Engine {
    /// Starts a round of `signal_layers` beside the call loop, unless one is still running. The
    /// messages are RPCs, and the call loop must not wait on them.
    pub(crate) fn spawn_signal_layers(&self, room: &Arc<Room>) {
        if room.with_layers(|l| std::mem::replace(&mut l.busy, true)) {
            return;
        }
        let engine = self.clone();
        let weak = Arc::downgrade(room);
        self.spawn_task(async move {
            if let Some(room) = weak.upgrade() {
                engine.signal_layers(&room).await;
                room.with_layers(|l| l.busy = false);
            }
        });
    }

    /// Sends what changed since the last check: this device's tile widths and layer tables to its
    /// forwarder, and, on a forwarder, the masks each sender should encode. Runs on the membership
    /// tick of a forwarded call.
    async fn signal_layers(&self, room: &Arc<Room>) {
        // A test's stand-in for a released device knows nothing of layers.
        if self.inner.legacy_calls.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let Some(forwarding) = room.forwarding() else {
            return;
        };
        let me = room.my_sid();
        let forwarder = forwarding.forwarder_sid();
        // A capture turned on after the forwarder asked starts on one stream. It takes the mask
        // asked for, since the forwarder sends a mask only when it changes.
        if self.simulcast_on() {
            for screen in [false, true] {
                let asked = room.with_layers(|l| l.wanted_here[usize::from(screen)]);
                if asked.is_some() && room.wanted_layers(screen) != asked {
                    room.set_wanted_layers(screen, asked);
                }
            }
        }
        if forwarding.role() == Role::Leaf || forwarding.role() == Role::Root {
            self.tell_tiles(room, forwarder).await;
            // Only a sender whose capture encodes layers announces them.
            if self.simulcast_on() {
                self.tell_sender_layers(room, forwarder).await;
            }
        }
        if forwarding.role() != Role::Leaf {
            self.tell_wanted(room, me).await;
        }
    }

    /// Whether this device's captures send simulcast layers. See `Inner::simulcast`.
    pub(crate) fn simulcast_on(&self) -> bool {
        self.inner.simulcast.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The device behind sender id `sid` in this call, when it supports layers.
    fn layer_peer(&self, room: &Room, sid: u64) -> Option<DeviceId> {
        if !room.peer_supports(sid, crate::calls::FEATURE_VIDEO_LAYERS) {
            return None;
        }
        room.device_of(sid)
    }

    async fn tell_tiles(&self, room: &Arc<Room>, forwarder: u64) {
        let tiles: Vec<(u64, bool, u32)> = {
            let shown = crate::calls::lock(&self.inner.calls.tiles).clone();
            let mut out: Vec<(u64, bool, u32)> = shown
                .into_iter()
                .filter_map(|((root, screen), width)| room.sid_of_root(&root).map(|sid| (sid, screen, width)))
                .collect();
            out.sort_unstable();
            out
        };
        let changed = room.with_layers(|l| l.told_tiles.as_ref() != Some(&(forwarder, tiles.clone())));
        if !changed {
            return;
        }
        if forwarder == room.my_sid() {
            // The forwarder's own window: no message, straight into its relay.
            self.apply_tiles(room, forwarder, &tiles).await;
        } else {
            let Some(device) = self.layer_peer(room, forwarder) else {
                return;
            };
            let msg = CallMsg::TileWidths { room: room.id(), tiles: tiles.clone() };
            if !self.send_layer_msg(device, msg).await {
                return;
            }
        }
        room.with_layers(|l| l.told_tiles = Some((forwarder, tiles)));
    }

    async fn tell_sender_layers(&self, room: &Arc<Room>, forwarder: u64) {
        for (index, screen) in [(0usize, false), (1, true)] {
            let Some(table) = room.sending_layers(screen) else {
                continue;
            };
            let changed = room.with_layers(|l| l.told_layers[index].as_ref() != Some(&(forwarder, table.clone())));
            if !changed {
                continue;
            }
            if forwarder == room.my_sid() {
                self.apply_sender_layers(room, forwarder, screen, &table).await;
            } else {
                let Some(device) = self.layer_peer(room, forwarder) else {
                    continue;
                };
                let msg = CallMsg::VideoLayers { room: room.id(), screen, layers: table.clone() };
                if !self.send_layer_msg(device, msg).await {
                    continue;
                }
            }
            room.with_layers(|l| l.told_layers[index] = Some((forwarder, table)));
        }
    }

    /// On a forwarder: tells each sender the layers its viewers need, when that changed.
    async fn tell_wanted(&self, room: &Arc<Room>, me: u64) {
        let masks: Vec<((u64, bool), u8)> = {
            let Some(forwarding) = room.forwarding() else {
                return;
            };
            let sfu = forwarding.sfu().lock().await;
            let Some(sfu) = sfu.as_ref() else {
                return;
            };
            room.layered_streams()
                .into_iter()
                .map(|(sid, screen)| ((sid, screen), sfu.wanted_layers(sid, screen)))
                .collect()
        };
        for ((sid, screen), mask) in masks {
            let changed = room.with_layers(|l| l.told_wanted.get(&(sid, screen)) != Some(&mask));
            if !changed {
                continue;
            }
            if sid == me {
                self.apply_wanted(room, screen, mask);
            } else {
                let Some(device) = self.layer_peer(room, sid) else {
                    continue;
                };
                let msg = CallMsg::WantedLayers { room: room.id(), screen, mask };
                if !self.send_layer_msg(device, msg).await {
                    continue;
                }
            }
            room.with_layers(|l| {
                l.told_wanted.insert((sid, screen), mask);
            });
        }
    }

    async fn send_layer_msg(&self, device: DeviceId, msg: CallMsg) -> bool {
        match self.call_rpc(device, msg).await {
            Ok(CallReply::Ack) => true,
            Ok(other) => {
                debug!(device = %device, reply = ?other, "layer message refused");
                false
            }
            Err(e) => {
                debug!(device = %device, error = %e, "layer message not delivered");
                false
            }
        }
    }

    /// Puts a viewer's tile widths into this device's relay, and marks the viewer as knowing layers.
    pub(crate) async fn apply_tiles(&self, room: &Room, viewer: u64, tiles: &[(u64, bool, u32)]) -> bool {
        let Some(forwarding) = room.forwarding() else {
            return false;
        };
        let mut sfu = forwarding.sfu().lock().await;
        let Some(sfu) = sfu.as_mut() else {
            return false;
        };
        sfu.set_layer_aware(viewer, true);
        // The message is the viewer's whole set, so a tile it stopped drawing is dropped.
        sfu.clear_viewer_tiles(viewer);
        for (sender, screen, width) in tiles {
            sfu.set_viewer_tile(viewer, *sender, *screen, (*width).min(8192));
        }
        true
    }

    /// Puts a sender's layer table into this device's relay.
    pub(crate) async fn apply_sender_layers(&self, room: &Room, sender: u64, screen: bool, table: &[(u32, u32)]) -> bool {
        let Some(forwarding) = room.forwarding() else {
            return false;
        };
        let mut sfu = forwarding.sfu().lock().await;
        let Some(sfu) = sfu.as_mut() else {
            return false;
        };
        sfu.set_sender_layers(sender, screen, table.iter().map(|(width, kbps)| LayerInfo { width: *width, kbps: *kbps }).collect());
        room.note_layered_stream(sender, screen);
        true
    }

    /// Records the mask this device's forwarder asked for, and hands it to the capture, if it sends
    /// layers at all. A forwarder only asks a sender that announced layers.
    pub(crate) fn apply_wanted(&self, room: &Room, screen: bool, mask: u8) {
        room.with_layers(|l| l.wanted_here[usize::from(screen)] = Some(mask));
        if self.simulcast_on() {
            room.set_wanted_layers(screen, Some(mask));
        }
    }

    /// For tests: the layer of the last frame received of `root_id`'s camera or screen in the
    /// current call.
    pub(crate) fn received_layer_now(&self, root_id: &str, screen: bool) -> Option<u8> {
        let room = crate::calls::lock(&self.inner.calls.current).clone()?;
        let sid = room.sid_of_root(root_id)?;
        room.with_layers(|l| l.received.get(&(sid, screen)).copied())
    }

    /// For tests: the root id of the active speaker as this device's relay ranks them, the one a
    /// tight budget keeps sharp. `None` when this device does not forward or nobody speaks.
    pub(crate) async fn active_speaker_now(&self) -> Option<String> {
        let room = crate::calls::lock(&self.inner.calls.current).clone()?;
        let sid = {
            let sfu = room.forwarding()?.sfu().lock().await;
            sfu.as_ref()?.top_speakers().first().copied()?
        };
        room.root_of_sid(sid)
    }

    /// For tests: the layers `sender_root`'s camera announced to this device's relay, as
    /// (width, kbps), smallest first.
    pub(crate) async fn sender_layers_now(&self, sender_root: &str) -> Vec<(u32, u32)> {
        let Some(room) = crate::calls::lock(&self.inner.calls.current).clone() else {
            return Vec::new();
        };
        let (Some(sender), Some(forwarding)) = (room.sid_of_root(sender_root), room.forwarding()) else {
            return Vec::new();
        };
        let sfu = forwarding.sfu().lock().await;
        sfu.as_ref()
            .map(|sfu| sfu.sender_layers(sender, false).iter().map(|l| (l.width, l.kbps)).collect())
            .unwrap_or_default()
    }

    /// For tests: sets the video budget of `viewer_root`'s downlink on this device's relay, as a
    /// measured estimate would. Returns false when this device does not forward the call.
    pub(crate) async fn set_viewer_budget_now(&self, viewer_root: &str, kbps: u32) -> bool {
        let Some(room) = crate::calls::lock(&self.inner.calls.current).clone() else {
            return false;
        };
        let Some(viewer) = room.sid_of_root(viewer_root) else {
            return false;
        };
        let Some(forwarding) = room.forwarding() else {
            return false;
        };
        let mut sfu = forwarding.sfu().lock().await;
        let Some(sfu) = sfu.as_mut() else {
            return false;
        };
        sfu.set_listener_budget(viewer, kbps);
        true
    }

    /// Answers `TileWidths` from `from`. Only the forwarder of a forwarded call takes it.
    pub(crate) async fn on_tile_widths(&self, from: DeviceId, id: RoomId, tiles: Vec<(u64, bool, u32)>) -> CallReply {
        let Some(room) = self.current_room(&id) else {
            return CallReply::Refused("not in this call".into());
        };
        let Some(viewer) = room.live_sid(&from) else {
            return CallReply::Refused("the sender is not in this call".into());
        };
        if self.apply_tiles(&room, viewer, &tiles).await {
            CallReply::Ack
        } else {
            CallReply::Refused("this device does not forward this call".into())
        }
    }

    /// Answers `VideoLayers` from `from`. Only the forwarder of a forwarded call takes it.
    pub(crate) async fn on_video_layers(&self, from: DeviceId, id: RoomId, screen: bool, layers: Vec<(u32, u32)>) -> CallReply {
        let Some(room) = self.current_room(&id) else {
            return CallReply::Refused("not in this call".into());
        };
        let Some(sender) = room.live_sid(&from) else {
            return CallReply::Refused("the sender is not in this call".into());
        };
        if self.apply_sender_layers(&room, sender, screen, &layers).await {
            CallReply::Ack
        } else {
            CallReply::Refused("this device does not forward this call".into())
        }
    }

    /// Answers `WantedLayers` from `from`, which must be this device's forwarder.
    pub(crate) fn on_wanted_layers(&self, from: DeviceId, id: RoomId, screen: bool, mask: u8) -> CallReply {
        let Some(room) = self.current_room(&id) else {
            return CallReply::Refused("not in this call".into());
        };
        let Some(forwarding) = room.forwarding() else {
            return CallReply::Refused("this call is not forwarded".into());
        };
        if room.live_sid(&from) != Some(forwarding.forwarder_sid()) {
            return CallReply::Refused("only the forwarder asks for layers".into());
        }
        self.apply_wanted(&room, screen, mask);
        CallReply::Ack
    }
}
