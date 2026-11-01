//! Avatar decorations: an overlay around a person's picture, a built-in piece of art named by its id or a picture they
//! uploaded. It travels like the banner, without touching any older encoding: as `Payload::AvatarDecorationSet` in each
//! community the person is in, carried so an older client shows the event as locked, and as a
//! `Request::AvatarDecoration` to contacts, which an older device answers with an error. The newest by HLC wins.
//!
//! A built-in is matched by id at the viewer's end, so nothing is fetched. An upload is untrusted art from someone
//! else, and the window would decode it on every row it shows on, so this device decodes every frame first, off the
//! async threads, under the caps below. Only art that passes gets a file. That file has a name of its own, so a blob
//! fetched through another path (a banner naming the same hash) is never shown as a decoration unchecked. A still PNG
//! made here from the art goes beside it, for rows that hold still and for reduced motion.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::time::{Duration, Instant};

use dissent_events::{ChannelId, Decoration, Hlc, Payload, Picture, MAX_DECORATION_BYTES};
use dissent_identity::{DeviceCert, DeviceId, DeviceKey, RootId, Signature};
use image::codecs::gif::GifDecoder;
use image::codecs::png::PngDecoder;
use image::codecs::webp::WebPDecoder;
use image::{AnimationDecoder, DynamicImage, ImageDecoder, ImageFormat, Limits, RgbaImage};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::avatars::{extension_of, peers_vault, stem, write_sealed, Owner};
use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::presence::beacon_devices;
use crate::rpc::{Request, Response};
use crate::store::Paths;
use crate::world::World;

const DECORATION_DOMAIN: &[u8] = b"dissent/avatar-decoration/v1";

/// Longest side of an uploaded decoration, in pixels. The art is shown at most a little over 100 px across.
pub const MAX_DECORATION_SIDE: u32 = 512;
/// Most frames an uploaded decoration may have.
pub const MAX_DECORATION_FRAMES: usize = 300;
/// Most pixels all of a decoration's frames may hold together: 300 frames of 288 by 288, what a viewer's window would
/// keep decoded in the worst case is about 100 MB.
const MAX_DECODED_PIXELS: u64 = 300 * 288 * 288;
/// How far the shorter side may be from the longer, as a share of it, for the art to stay centred on a round picture.
const SQUAREISH_PERCENT: u32 = 90;
/// How many past uploads this device keeps offering.
const MAX_RECENT: usize = 6;
/// How long decoding one decoration may take before it is refused.
const MAX_DECODE_TIME: Duration = Duration::from_secs(10);
/// Most memory one frame may take while it is decoded.
const MAX_FRAME_ALLOC: u64 = 64 * 1024 * 1024;

/// This device's decoration, and those contacts sent directly, each with its HLC. Kept sealed in `decorations.bin`, as
/// the picture keys inside must not be readable at rest. `own` is `None` until this device first sets or clears one.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DecorationBook {
    pub own: Option<(Hlc, Option<Decoration>)>,
    pub peers: BTreeMap<RootId, (Hlc, Option<Decoration>)>,
    /// This device's latest uploads, newest first, so one can be picked again without its file.
    pub recent: Vec<Picture>,
}

impl DecorationBook {
    /// The uploads of this device's own: the one in use and the recent ones.
    pub(crate) fn own_pictures(&self) -> impl Iterator<Item = Picture> + '_ {
        let current = self.own.iter().filter_map(|(_, d)| d.as_ref()?.picture());
        current.chain(self.recent.iter().copied())
    }

    /// Every uploaded picture this book names.
    pub(crate) fn pictures(&self) -> impl Iterator<Item = Picture> + '_ {
        self.own_pictures().chain(self.peers.values().filter_map(|(_, d)| d.as_ref()?.picture()))
    }
}

/// One of this device's past uploads, as the picker shows it. `id` picks it again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RecentDecoration {
    pub id: String,
    pub decoration_image: String,
    pub decoration_poster: Option<String>,
}

/// A person's decoration as the views carry it. Either `builtin` names art the UI ships, or the two paths are this
/// device's checked copy of an upload and its still frame. A person whose upload is not here yet has no view.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DecorationView {
    pub builtin: Option<String>,
    pub decoration_image: Option<String>,
    pub decoration_poster: Option<String>,
}

/// A decoration sent directly to a contact's device, signed by that device, as `SignedBanner` is.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SignedDecoration {
    pub root: RootId,
    pub device: DeviceId,
    pub hlc: Hlc,
    pub decoration: Option<Decoration>,
    pub cert: DeviceCert,
    pub sig: Signature,
}

fn signing_bytes(root: RootId, device: DeviceId, hlc: Hlc, decoration: &Option<Decoration>) -> Result<Vec<u8>> {
    let body = postcard::to_allocvec(&(root, device, hlc, decoration)).map_err(|e| CoreError::Encoding(e.to_string()))?;
    Ok([DECORATION_DOMAIN, body.as_slice()].concat())
}

impl SignedDecoration {
    pub(crate) fn sign(key: &DeviceKey, cert: DeviceCert, hlc: Hlc, decoration: Option<Decoration>) -> Result<Self> {
        let (root, device) = (cert.root, cert.device);
        let sig = key.sign(&signing_bytes(root, device, hlc, &decoration)?);
        Ok(Self { root, device, hlc, decoration, cert, sig })
    }

    /// Checks the cert, that it binds the named device and root, the device signature, and the caps.
    pub(crate) fn verify(&self) -> Result<()> {
        self.cert.verify()?;
        if self.cert.device != self.device || self.cert.root != self.root {
            return Err(CoreError::Invalid("decoration does not match its certificate".into()));
        }
        self.device.verify(&signing_bytes(self.root, self.device, self.hlc, &self.decoration)?, &self.sig)?;
        if self.decoration.as_ref().is_some_and(|d| !d.is_valid()) {
            return Err(CoreError::Invalid("the decoration is out of range".into()));
        }
        Ok(())
    }
}

fn refused(why: impl std::fmt::Display) -> CoreError {
    CoreError::Invalid(format!("avatar decoration: {why}"))
}

/// What decoding a decoration found, and the still frame it made.
#[derive(Clone, Debug)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CheckedArt {
    pub width: u32,
    pub height: u32,
    pub frames: usize,
    /// A PNG of the first frame that shows anything, so art that fades in does not rest invisible.
    pub poster: Vec<u8>,
}

/// Decodes `bytes` as a WebP, GIF or PNG, animated or still, whatever its name says, and checks it against the caps. It
/// takes real time on large art, so it is called off the async threads.
pub(crate) fn check_art(bytes: &[u8]) -> Result<CheckedArt> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_DECORATION_BYTES {
        return Err(refused(format!("it must be at most {} MB", MAX_DECORATION_BYTES / (1024 * 1024))));
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DECORATION_SIDE);
    limits.max_image_height = Some(MAX_DECORATION_SIDE);
    limits.max_alloc = Some(MAX_FRAME_ALLOC);
    let too_large = || refused(format!("it must be at most {MAX_DECORATION_SIDE} by {MAX_DECORATION_SIDE} pixels"));
    let source = Cursor::new(bytes);
    if bytes.starts_with(b"GIF8") {
        let mut decoder = GifDecoder::new(source).map_err(refused)?;
        decoder.set_limits(limits).map_err(|_| too_large())?;
        let (w, h) = decoder.dimensions();
        check_size(w, h)?;
        walk(w, h, decoder.into_frames())
    } else if bytes.starts_with(b"\x89PNG") {
        let decoder = PngDecoder::with_limits(source, limits).map_err(|_| too_large())?;
        let (w, h) = decoder.dimensions();
        check_size(w, h)?;
        if decoder.is_apng().map_err(refused)? {
            walk(w, h, decoder.apng().map_err(refused)?.into_frames())
        } else {
            still(w, h, DynamicImage::from_decoder(decoder).map_err(refused)?)
        }
    } else if bytes.len() > 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        let mut decoder = WebPDecoder::new(source).map_err(refused)?;
        decoder.set_limits(limits).map_err(|_| too_large())?;
        let (w, h) = decoder.dimensions();
        check_size(w, h)?;
        if decoder.has_animation() {
            walk(w, h, decoder.into_frames())
        } else {
            still(w, h, DynamicImage::from_decoder(decoder).map_err(refused)?)
        }
    } else {
        Err(refused("it must be a WebP, GIF or PNG"))
    }
}

fn check_size(w: u32, h: u32) -> Result<()> {
    if w == 0 || h == 0 || w > MAX_DECORATION_SIDE || h > MAX_DECORATION_SIDE {
        return Err(refused(format!("it must be at most {MAX_DECORATION_SIDE} by {MAX_DECORATION_SIDE} pixels")));
    }
    if w.min(h) * 100 < w.max(h) * SQUAREISH_PERCENT {
        return Err(refused("it must be about square"));
    }
    Ok(())
}

fn encode_poster(frame: RgbaImage) -> Result<Vec<u8>> {
    let mut poster = Vec::new();
    DynamicImage::ImageRgba8(frame).write_to(&mut Cursor::new(&mut poster), ImageFormat::Png).map_err(refused)?;
    Ok(poster)
}

fn still(width: u32, height: u32, image: DynamicImage) -> Result<CheckedArt> {
    let poster = encode_poster(image.to_rgba8())?;
    Ok(CheckedArt { width, height, frames: 1, poster })
}

/// Decodes every frame, one at a time so only one is ever held, and stops at the first cap passed.
fn walk(width: u32, height: u32, frames: image::Frames<'_>) -> Result<CheckedArt> {
    let started = Instant::now();
    let mut count = 0usize;
    let mut first: Option<RgbaImage> = None;
    let mut shown = false;
    for frame in frames {
        let frame = frame.map_err(refused)?;
        count += 1;
        if count > MAX_DECORATION_FRAMES {
            return Err(refused(format!("it must have at most {MAX_DECORATION_FRAMES} frames")));
        }
        if count as u64 * u64::from(width) * u64::from(height) > MAX_DECODED_PIXELS {
            return Err(refused("it has too many frames for its size"));
        }
        if started.elapsed() > MAX_DECODE_TIME {
            return Err(refused("it takes too long to decode"));
        }
        if !shown {
            let buffer = frame.into_buffer();
            shown = buffer.pixels().any(|p| p.0[3] != 0);
            if shown || first.is_none() {
                first = Some(buffer);
            }
        }
    }
    let first = first.ok_or_else(|| refused("it has no frames"))?;
    Ok(CheckedArt { width, height, frames: count, poster: encode_poster(first)? })
}

/// `bytes` without the metadata its format can carry: a PNG's text, EXIF and time chunks, a WebP's EXIF and XMP chunks,
/// a GIF's comments and application blocks other than its loop count. The pixels are kept as they are, since a canvas
/// or a GIF re-encode would lose the animation or the alpha. A file these walks cannot follow is refused.
pub(crate) fn strip_metadata(bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.starts_with(b"GIF8") {
        strip_gif(bytes)
    } else if bytes.starts_with(b"\x89PNG") {
        strip_png(bytes)
    } else if bytes.starts_with(b"RIFF") {
        strip_webp(bytes)
    } else {
        Err(refused("it must be a WebP, GIF or PNG"))
    }
}

fn cut() -> CoreError {
    refused("the file is cut short")
}

fn strip_png(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut out = bytes.get(..8).ok_or_else(cut)?.to_vec();
    let mut at = 8;
    loop {
        let head = bytes.get(at..at + 8).ok_or_else(cut)?;
        let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let kind = &head[4..8];
        let end = at.checked_add(12).and_then(|n| n.checked_add(len)).ok_or_else(cut)?;
        let chunk = bytes.get(at..end).ok_or_else(cut)?;
        if !matches!(kind, b"tEXt" | b"zTXt" | b"iTXt" | b"eXIf" | b"tIME") {
            out.extend_from_slice(chunk);
        }
        if kind == b"IEND" {
            return Ok(out);
        }
        at = end;
    }
}

fn strip_webp(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut body = bytes.get(8..12).ok_or_else(cut)?.to_vec();
    let mut at = 12;
    while at < bytes.len() {
        let head = bytes.get(at..at + 8).ok_or_else(cut)?;
        let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
        // A chunk's data is padded to an even length.
        let end = at.checked_add(8).and_then(|n| n.checked_add(len)).and_then(|n| n.checked_add(len & 1)).ok_or_else(cut)?;
        let chunk = bytes.get(at..end).ok_or_else(cut)?;
        match &head[..4] {
            b"EXIF" | b"XMP " => {}
            b"VP8X" => {
                let mut extended = chunk.to_vec();
                // The extended header's flags say which chunks follow.
                *extended.get_mut(8).ok_or_else(cut)? &= !0b0000_1100;
                body.extend_from_slice(&extended);
            }
            _ => body.extend_from_slice(chunk),
        }
        at = end;
    }
    let size = u32::try_from(body.len()).map_err(|_| cut())?;
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

fn strip_gif(bytes: &[u8]) -> Result<Vec<u8>> {
    /// The end of the sub-blocks that start at `at`, past their terminator.
    fn after_blocks(bytes: &[u8], mut at: usize) -> Result<usize> {
        loop {
            let len = *bytes.get(at).ok_or_else(cut)? as usize;
            at += 1 + len;
            if len == 0 {
                return if at <= bytes.len() { Ok(at) } else { Err(cut()) };
            }
        }
    }
    let table = |flags: u8| if flags & 0x80 != 0 { 3usize << ((flags & 7) + 1) } else { 0 };
    let flags = *bytes.get(10).ok_or_else(cut)?;
    let mut at = 13 + table(flags);
    let mut out = bytes.get(..at).ok_or_else(cut)?.to_vec();
    loop {
        match *bytes.get(at).ok_or_else(cut)? {
            0x3B => {
                out.push(0x3B);
                return Ok(out);
            }
            0x21 => {
                let label = *bytes.get(at + 1).ok_or_else(cut)?;
                let end = after_blocks(bytes, at + 2)?;
                let keep = match label {
                    0xFE => false,
                    0xFF => bytes.get(at + 3..at + 14).is_some_and(|name| name == b"NETSCAPE2.0"),
                    _ => true,
                };
                if keep {
                    out.extend_from_slice(&bytes[at..end]);
                }
                at = end;
            }
            0x2C => {
                let flags = *bytes.get(at + 9).ok_or_else(cut)?;
                // The descriptor, its colour table and the LZW code size, then the data.
                let end = after_blocks(bytes, at + 10 + table(flags) + 1)?;
                out.extend_from_slice(&bytes[at..end]);
                at = end;
            }
            _ => return Err(refused("the file is not a well-formed GIF")),
        }
    }
}

/// The checked copy of an uploaded decoration on this device, if any.
fn art_file(paths: &Paths, picture: &Picture) -> Option<String> {
    let dir = paths.avatars_dir();
    ["webp", "png", "gif"]
        .iter()
        .map(|ext| dir.join(format!("{}-deco.{ext}", stem(picture))))
        .find(|path| path.is_file())
        .map(|path| path.to_string_lossy().into_owned())
}

fn poster_file(paths: &Paths, picture: &Picture) -> Option<String> {
    let path = paths.avatars_dir().join(format!("{}-deco-still.png", stem(picture)));
    path.is_file().then(|| path.to_string_lossy().into_owned())
}

/// Keeps checked art sealed beside the other pictures, the still first, so a view that finds the art finds both.
fn keep_art(paths: &Paths, picture: &Picture, bytes: &[u8], poster: &[u8]) -> Result<()> {
    let dir = paths.avatars_dir();
    paths.write_bytes(&dir.join(format!("{}-deco-still.png", stem(picture))), poster)?;
    paths.write_bytes(&dir.join(format!("{}-deco.{}", stem(picture), extension_of(bytes))), bytes)?;
    Ok(())
}

impl World {
    /// The decoration a person shows: this device's own, or the newest one a community or a direct message gave us.
    pub(crate) fn avatar_decoration(&self, root: &RootId) -> Option<Decoration> {
        if *root == self.me {
            return self.decorations.own.as_ref().and_then(|(_, d)| d.clone());
        }
        let mut newest = self.decorations.peers.get(root);
        for c in self.communities.values() {
            if let Some(entry) = c.state().avatar_decorations.get(root) {
                if newest.is_none_or(|(hlc, _)| entry.0 > *hlc) {
                    newest = Some(entry);
                }
            }
        }
        newest.and_then(|(_, d)| d.clone())
    }

    pub(crate) fn decoration_view(&self, root: &RootId) -> Option<DecorationView> {
        match self.avatar_decoration(root)? {
            Decoration::Builtin(id) => Some(DecorationView { builtin: Some(id), decoration_image: None, decoration_poster: None }),
            Decoration::Upload(picture) => Some(DecorationView {
                builtin: None,
                decoration_image: Some(art_file(&self.paths, &picture)?),
                decoration_poster: poster_file(&self.paths, &picture),
            }),
        }
    }
}

impl Engine {
    /// Sets this device's decoration to the built-in art named `id`.
    pub async fn set_avatar_decoration_builtin(&self, id: String) -> Result<()> {
        let decoration = Decoration::Builtin(id);
        if !decoration.is_valid() {
            return Err(refused("that is not a decoration id"));
        }
        self.publish_decoration(Some(decoration)).await
    }

    /// Sets this device's decoration to uploaded art: a WebP, GIF or PNG, animated or still, about square, at most 2 MB,
    /// 512 pixels a side and 300 frames. It is refused, not re-encoded, beyond that. Metadata is removed.
    pub async fn set_avatar_decoration(&self, bytes: Vec<u8>) -> Result<()> {
        let (bytes, art) = tokio::task::spawn_blocking(move || {
            if bytes.len() as u64 > MAX_DECORATION_BYTES {
                return Err(refused(format!("it must be at most {} MB", MAX_DECORATION_BYTES / (1024 * 1024))));
            }
            let bytes = strip_metadata(&bytes)?;
            let art = check_art(&bytes)?;
            Ok((bytes, art))
        })
        .await
        .map_err(|e| refused(format!("the check stopped: {e}")))??;
        let picture = self.add_picture_blob(&bytes).await?;
        keep_art(&self.paths(), &picture, &bytes, &art.poster)?;
        self.publish_decoration(Some(Decoration::Upload(picture))).await
    }

    /// The uploads this device can offer again, newest first. Only art it still holds a checked copy of.
    pub fn recent_decorations(&self) -> Vec<RecentDecoration> {
        let w = self.read();
        w.decorations
            .recent
            .iter()
            .filter_map(|p| {
                Some(RecentDecoration {
                    id: stem(p),
                    decoration_image: art_file(&w.paths, p)?,
                    decoration_poster: poster_file(&w.paths, p),
                })
            })
            .collect()
    }

    /// Sets this device's decoration to one of its recent uploads, named as `recent_decorations` names it.
    pub async fn set_avatar_decoration_recent(&self, id: String) -> Result<()> {
        let picture = self.read().decorations.recent.iter().find(|p| stem(p) == id).copied();
        let picture = picture.ok_or_else(|| CoreError::NotFound("recent decoration".into()))?;
        self.publish_decoration(Some(Decoration::Upload(picture))).await
    }

    /// Removes this device's decoration, everywhere it was published.
    pub async fn clear_avatar_decoration(&self) -> Result<()> {
        self.publish_decoration(None).await
    }

    pub(crate) async fn publish_decoration(&self, decoration: Option<Decoration>) -> Result<()> {
        let hlc = self.stamp();
        let communities: Vec<_> = {
            let mut w = self.write();
            w.decorations.own = Some((hlc, decoration.clone()));
            if let Some(picture) = decoration.as_ref().and_then(|d| d.picture()) {
                w.decorations.recent.retain(|p| *p != picture);
                w.decorations.recent.insert(0, picture);
                w.decorations.recent.truncate(MAX_RECENT);
            }
            write_sealed(&w.paths.decorations(), &peers_vault(&self.inner.vault), &w.decorations)?;
            w.communities.keys().copied().collect()
        };
        for community in communities {
            let payload = Payload::AvatarDecorationSet { decoration: decoration.clone() };
            if let Err(e) = self.author_publish(community, ChannelId::CONTROL, payload).await {
                warn!(community = %community, error = %e, "avatar decoration not published to a community");
            }
        }
        self.send_decoration_direct();
        self.emit_owner(Owner::Person(self.inner.me));
        Ok(())
    }

    /// Our decoration, signed now under the HLC it was set at, if this device ever set one. Signing again does not make
    /// it newer, so a sync cannot bring an old choice back over a newer one.
    fn own_decoration(&self) -> Option<Box<SignedDecoration>> {
        let (hlc, decoration) = self.read().decorations.own.clone()?;
        let (key, cert) = (&self.inner.identity.device, self.inner.identity.cert.clone());
        match SignedDecoration::sign(key, cert, hlc, decoration) {
            Ok(signed) => Some(Box::new(signed)),
            Err(e) => {
                warn!(error = %e, "could not sign our decoration");
                None
            }
        }
    }

    /// Sends the signed decoration to each contact device, over the direct connection, in the background.
    fn send_decoration_direct(&self) {
        let bytes = match self.request_bytes(&Request::AvatarDecoration(self.own_decoration())) {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!(error = %e, "could not encode our decoration");
                return;
            }
        };
        let devices = beacon_devices(&self.read());
        let node = self.node();
        for device in devices {
            let node = node.clone();
            let bytes = bytes.clone();
            self.spawn_task(async move {
                if let Err(e) = node.request(device, bytes).await {
                    debug!(device = %device, error = %e, "decoration not delivered directly");
                }
            });
        }
    }

    /// Part of a contact's profile exchange: sends ours and takes theirs. A device from before decorations answers with
    /// an error, and simply has none.
    pub(crate) async fn exchange_decorations(&self, device: DeviceId) {
        match self.call(device, &Request::AvatarDecoration(self.own_decoration())).await {
            Ok(Response::AvatarDecoration(Some(theirs))) => {
                if theirs.device != device {
                    debug!(device = %device, "a decoration signed by another device was not taken");
                } else if let Err(e) = self.accept_decoration(*theirs) {
                    debug!(device = %device, error = %e, "synced decoration not taken");
                }
            }
            Ok(_) => {}
            Err(e) => debug!(device = %device, error = %e, "decoration not exchanged"),
        }
    }

    /// Answers a contact's `AvatarDecoration`: takes theirs, and sends ours. An upload's key is only for contacts, so
    /// anyone else gets an error whatever they send.
    pub(crate) fn answer_decoration(&self, from: DeviceId, sd: Option<SignedDecoration>) -> Response {
        let signed = sd.is_some();
        if let Some(sd) = sd {
            if sd.device != from {
                return Response::Error("decoration is not from the asking device".into());
            }
            if let Err(e) = self.accept_decoration(sd) {
                return Response::Error(format!("invalid decoration: {e}"));
            }
        }
        // `accept_decoration` proved a signed asker is a contact. One with nothing to send is known by its device.
        if !signed && !beacon_devices(&self.read()).contains(&from) {
            return Response::Error("decoration asked for by someone who is not a contact".into());
        }
        Response::AvatarDecoration(self.own_decoration())
    }

    /// Accepts a decoration from a contact. The newest by HLC wins. An upload is fetched and checked in the background.
    pub(crate) fn accept_decoration(&self, sd: SignedDecoration) -> Result<()> {
        sd.verify()?;
        let root = sd.root;
        if root == self.inner.me {
            return Ok(());
        }
        let newer = {
            let mut guard = self.write();
            let w = &mut *guard;
            if !w.is_contact(&root) || w.contact_book.blocked.contains(&root) {
                return Err(CoreError::Invalid("decoration from someone who is not a contact".into()));
            }
            let newer = w.decorations.peers.get(&root).is_none_or(|(hlc, _)| sd.hlc > *hlc);
            if newer {
                w.decorations.peers.insert(root, (sd.hlc, sd.decoration.clone()));
                write_sealed(&w.paths.decorations(), &peers_vault(&self.inner.vault), &w.decorations)?;
            }
            newer
        };
        if newer {
            match sd.decoration.as_ref().and_then(|d| d.picture()) {
                Some(picture) => self.fetch_decoration(Owner::Person(root), picture, vec![root]),
                None => self.emit_owner(Owner::Person(root)),
            }
        }
        Ok(())
    }

    /// Fetches uploaded art in the background, unless this device has it, as `fetch_picture` does, then decodes it under
    /// the caps. Art that fails is dropped and not asked for again this run: the person shows no decoration.
    pub(crate) fn fetch_decoration(&self, owner: Owner, picture: Picture, roots: Vec<RootId>) {
        if art_file(&self.read().paths, &picture).is_some() {
            self.emit_owner(owner);
            return;
        }
        let engine = self.clone();
        self.spawn_task(async move {
            for _ in 0..crate::avatars::FETCH_TRIES {
                let providers = engine.providers_for(&roots);
                if !providers.is_empty() {
                    match engine.fetch_picture_bytes(&picture, &providers).await {
                        Ok(bytes) => {
                            match engine.check_and_keep(&picture, bytes).await {
                                Ok(()) => engine.emit_owner(owner),
                                Err(e) => warn!(error = %e, "a decoration was refused"),
                            }
                            return;
                        }
                        Err(e) => debug!(error = %e, "decoration not fetched yet"),
                    }
                }
                tokio::time::sleep(crate::avatars::FETCH_WAIT).await;
            }
            debug!("gave up fetching a decoration for now");
        });
    }

    async fn check_and_keep(&self, picture: &Picture, bytes: Vec<u8>) -> Result<()> {
        let (bytes, art) = tokio::task::spawn_blocking(move || check_art(&bytes).map(|art| (bytes, art)))
            .await
            .map_err(|e| refused(format!("the check stopped: {e}")))??;
        keep_art(&self.paths(), picture, &bytes, &art.poster)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dissent_identity::RootKey;
    use image::codecs::gif::{GifEncoder, Repeat};
    use image::{Delay, Frame, Rgba};

    fn frame(w: u32, h: u32, tag: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            // A clear middle, as real art has, and an edge that changes per frame.
            if x > w / 4 && x < 3 * w / 4 && y > h / 4 && y < 3 * h / 4 {
                Rgba([0, 0, 0, 0])
            } else {
                Rgba([(tag * 40 % 255) as u8, x as u8, y as u8, 255])
            }
        })
    }

    fn gif(frames: u32, w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = GifEncoder::new_with_speed(&mut out, 30);
            enc.set_repeat(Repeat::Infinite).unwrap();
            for i in 0..frames {
                enc.encode_frame(Frame::from_parts(frame(w, h, i), 0, 0, Delay::from_numer_denom_ms(50, 1))).unwrap();
            }
        }
        out
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        DynamicImage::ImageRgba8(frame(w, h, 1)).write_to(&mut Cursor::new(&mut out), ImageFormat::Png).unwrap();
        out
    }

    fn lossless_webp(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        DynamicImage::ImageRgba8(frame(w, h, 1)).write_to(&mut Cursor::new(&mut out), ImageFormat::WebP).unwrap();
        out
    }

    #[test]
    fn a_signed_decoration_verifies_and_a_changed_one_does_not() {
        let root = RootKey::generate();
        let device = DeviceKey::generate();
        let cert = root.certify(device.id(), 0);
        let hlc = Hlc { wall_ms: 5, counter: 0 };
        let upload = Decoration::Upload(Picture { hash: [1; 32], key: [2; 32], nonce_prefix: [3; 8], size: 900 * 1024 });
        let sd = SignedDecoration::sign(&device, cert, hlc, Some(upload)).unwrap();
        sd.verify().unwrap();
        let mut builtin = sd.clone();
        builtin.decoration = Some(Decoration::Builtin("koi_pond".into()));
        assert!(builtin.verify().is_err(), "a changed decoration must not verify");
        let mut older = sd.clone();
        older.hlc.wall_ms = 4;
        assert!(older.verify().is_err(), "a changed HLC must not verify");
        let stranger = RootKey::generate();
        let mut foreign = sd.clone();
        foreign.cert = stranger.certify(device.id(), 0);
        assert!(foreign.verify().is_err(), "another root's cert must not verify");
    }

    #[test]
    fn art_is_checked_by_decoding_it_whatever_it_starts_with() {
        let art = check_art(&gif(3, 288, 288)).unwrap();
        assert_eq!((art.width, art.height, art.frames), (288, 288, 3));
        assert!(art.poster.starts_with(b"\x89PNG"));
        assert_eq!(check_art(&png(200, 200)).unwrap().frames, 1);
        assert_eq!(check_art(&lossless_webp(200, 200)).unwrap().frames, 1);
        for junk in [
            b"GIF89a-not-really".to_vec(),
            b"\x89PNG\r\n\x1a\n-not-really".to_vec(),
            b"RIFF\0\0\0\0WEBPVP8 not-really".to_vec(),
            b"<svg xmlns='http://www.w3.org/2000/svg'/>".to_vec(),
            vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 0],
            Vec::new(),
        ] {
            assert!(check_art(&junk).is_err(), "{:?}", &junk[..junk.len().min(8)]);
        }
    }

    #[test]
    fn art_past_a_cap_is_refused_without_keeping_every_frame() {
        assert!(check_art(&png(513, 513)).is_err(), "too wide");
        assert!(check_art(&png(300, 200)).is_err(), "not square");
        assert!(check_art(&png(300, 280)).is_ok(), "square enough");
        assert!(check_art(&gif(MAX_DECORATION_FRAMES as u32 + 1, 16, 16)).is_err(), "too many frames");
        assert!(check_art(&gif(MAX_DECORATION_FRAMES as u32, 16, 16)).is_ok());
        // Few enough frames, but too many at this size.
        assert!(check_art(&gif(200, 400, 400)).is_err(), "too many pixels");
        let mut big = png(16, 16);
        big.resize(MAX_DECORATION_BYTES as usize + 1, 0);
        assert!(check_art(&big).is_err(), "too many bytes");
    }

    #[test]
    fn the_still_is_the_first_frame_that_shows_anything() {
        let mut out = Vec::new();
        {
            let mut enc = GifEncoder::new_with_speed(&mut out, 30);
            enc.set_repeat(Repeat::Infinite).unwrap();
            let clear = RgbaImage::from_pixel(32, 32, Rgba([0, 0, 0, 0]));
            enc.encode_frame(Frame::from_parts(clear, 0, 0, Delay::from_numer_denom_ms(50, 1))).unwrap();
            enc.encode_frame(Frame::from_parts(frame(32, 32, 2), 0, 0, Delay::from_numer_denom_ms(50, 1))).unwrap();
        }
        let poster = image::load_from_memory(&check_art(&out).unwrap().poster).unwrap().to_rgba8();
        assert!(poster.pixels().any(|p| p.0[3] != 0), "a clear first frame is skipped");
    }

    #[test]
    fn metadata_is_removed_and_the_art_still_decodes() {
        // A GIF with a comment and a foreign application block.
        let plain = gif(2, 32, 32);
        let flags = plain[10];
        let at = 13 + if flags & 0x80 != 0 { 3 << ((flags & 7) + 1) } else { 0 };
        let mut noisy = plain[..at].to_vec();
        noisy.extend_from_slice(&[0x21, 0xFE, 5, b's', b'e', b'c', b'r', b't', 0]);
        noisy.extend_from_slice(&[0x21, 0xFF, 11]);
        noisy.extend_from_slice(b"XMP DataXMP");
        noisy.extend_from_slice(&[5, b's', b'e', b'c', b'r', b't', 0]);
        noisy.extend_from_slice(&plain[at..]);
        let clean = strip_metadata(&noisy).unwrap();
        assert!(!clean.windows(5).any(|w| w == b"secrt"));
        assert!(clean.windows(11).any(|w| w == b"NETSCAPE2.0"), "the loop count stays");
        assert_eq!(check_art(&clean).unwrap().frames, 2);

        // A PNG with a text chunk.
        let plain = png(32, 32);
        let mut noisy = plain[..33].to_vec();
        noisy.extend_from_slice(&5u32.to_be_bytes());
        noisy.extend_from_slice(b"tEXtsecrt\0\0\0\0");
        noisy.extend_from_slice(&plain[33..]);
        let clean = strip_metadata(&noisy).unwrap();
        assert!(!clean.windows(5).any(|w| w == b"secrt"));
        assert_eq!(clean, plain);

        // A WebP with an EXIF chunk, in the extended form. The size in its header follows.
        let inner = lossless_webp(32, 32);
        let mut noisy = inner[..12].to_vec();
        noisy.extend_from_slice(b"VP8X");
        noisy.extend_from_slice(&10u32.to_le_bytes());
        noisy.extend_from_slice(&[0b0000_1000, 0, 0, 0, 31, 0, 0, 31, 0, 0]);
        noisy.extend_from_slice(&inner[12..]);
        noisy.extend_from_slice(b"EXIF");
        noisy.extend_from_slice(&5u32.to_le_bytes());
        noisy.extend_from_slice(b"secrt\0");
        let clean = strip_metadata(&noisy).unwrap();
        assert!(!clean.windows(5).any(|w| w == b"secrt"));
        assert_eq!(clean[20] & 0b0000_1100, 0, "the flag is gone with the chunk");
        assert_eq!(u32::from_le_bytes([clean[4], clean[5], clean[6], clean[7]]) as usize, clean.len() - 8);
        assert_eq!(check_art(&clean).unwrap().frames, 1);

        assert!(strip_metadata(&gif(1, 16, 16)[..20]).is_err(), "a file cut short is refused");
    }

    /// Checks the art named by `DISSENT_DECORATIONS_DIR` against the upload caps, and prints what it holds. A built-in
    /// need not pass, since it ships with the app. This shows whether the caps fit real art.
    #[test]
    #[ignore = "reads a local folder"]
    fn real_art_against_the_caps() {
        let dir = std::env::var("DISSENT_DECORATIONS_DIR").expect("DISSENT_DECORATIONS_DIR");
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let bytes = std::fs::read(&path).unwrap();
            let started = Instant::now();
            match check_art(&bytes) {
                Ok(a) => println!("{path:?}: {}x{} {} frames in {:?}", a.width, a.height, a.frames, started.elapsed()),
                Err(e) => println!("{path:?}: {e}"),
            }
        }
    }
}
