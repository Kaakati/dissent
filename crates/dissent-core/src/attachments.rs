//! Files shared in a conversation or a channel.
//!
//! A file is encrypted and stored by `dissent-files`. Its `FileRef` (the hash, the key and the
//! name) travels inside the sealed message text, behind a marker, so the key is only ever sent
//! sealed. Downloads fetch the ciphertext from any device that may have it, then decrypt it into
//! the downloads directory. Seeders that opted in mirror the ciphertext, within `MirrorPolicy`.
//!
//! Known gap: seeders are members, so they can open the sealed text and see the file keys. The
//! ciphertext-only seeding that `seed` promises needs keys wrapped per recipient. Until then, the
//! seed flag only controls whether members mirror the file.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use dissent_files::{
    is_preview_image, is_thumbnail, make_thumbnail, pieces_of, FileRef, FilesError, Input,
    MirrorPolicy, SourceCount, Thumbnail, MAX_FILE_SIZE, PREVIEW_IMAGE_MAX_BYTES, PREVIEW_IMAGE_SIDE,
    THUMB_LONG_SIDE, THUMB_SOURCE_MAX,
};
use dissent_identity::RootId;
use dissent_net::PeerAddr;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::engine::{now_ms, Engine};
use crate::error::{CoreError, Result};
use crate::views::EngineEvent;
use crate::world::World;

/// Starts the marked payload. A typed message cannot contain it, because `clean_message` refuses it.
pub(crate) const MARK: &str = "\u{1e}dissent-attachments/v1\u{1e}";

/// One file in a message, with what the seeders need to know.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Attached {
    pub file: FileRef,
    /// The root that shared the file. Its devices are the first providers.
    pub from: RootId,
    /// Whether members may mirror the ciphertext.
    pub seed: bool,
}

#[derive(Serialize, Deserialize)]
struct Packed {
    text: String,
    files: Vec<Attached>,
    embeds: Vec<crate::previews::Embed>,
}

/// A thumbnail for one image in a message: a small JPEG of the file whose hash is `of`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Thumb {
    pub of: [u8; 32],
    pub file: FileRef,
    pub width: u32,
    pub height: u32,
}

/// Written after `Packed` when a message has extensions. A reader that stops after `Packed` never
/// looks at it, so older clients show the message and its files, without the extensions. Each
/// extension is a tag and its own postcard body, so a reader skips the tags it does not know.
#[derive(Serialize, Deserialize)]
struct Trailer {
    exts: Vec<(u8, Vec<u8>)>,
}

/// The tag of an extension holding a message's thumbnails: a postcard `Vec<Thumb>`.
const EXT_THUMBS: u8 = 1;

/// The tag of an extension holding the people a message mentions: a postcard [`Mentions`].
const EXT_MENTIONS: u8 = 2;

/// The tag of an extension holding the pictures and theme colours of link previews: a postcard
/// `Vec<EmbedExtra>`. Each entry names its preview by position, so the sealed `Embed` stays as it was.
const EXT_EMBEDS: u8 = 3;

/// Keeps a link preview's picture on this device, under its attachment id, where the thumbnails are.
pub(crate) fn keep_picture(paths: &crate::store::Paths, file: &FileRef, jpeg: &[u8]) -> Result<()> {
    std::fs::create_dir_all(paths.thumbnails_dir())?;
    std::fs::write(thumb_path(paths, &attachment_id(file)), jpeg)?;
    Ok(())
}

/// The picture of a link preview: a re-encoded JPEG blob, and its size.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EmbedImage {
    pub file: FileRef,
    pub width: u32,
    pub height: u32,
}

/// What a link preview has beyond its sealed fields: its picture, and its site's theme colour.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EmbedExtra {
    /// Position of the preview in the message's `embeds`.
    pub index: u32,
    pub image: Option<EmbedImage>,
    pub theme: Option<[u8; 3]>,
}

/// Who a message mentions. `users` are canonical roots; `everyone` is `@everyone`. A reader keeps a
/// user only if they are a member, and keeps `everyone` only if the author could mention everyone
/// when the message was accepted. The text itself stays plain `@Name`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mentions {
    pub users: Vec<[u8; 32]>,
    pub everyone: bool,
}

impl Mentions {
    /// True when there is nothing to attach.
    pub fn is_empty(&self) -> bool {
        self.users.is_empty() && !self.everyone
    }
}

/// Wraps message text and its files into the sealed payload.
#[cfg(test)]
pub(crate) fn pack(text: &str, files: &[Attached]) -> Result<String> {
    pack_all(text, files, &[], &[], None)
}

/// Packs text, files, link previews and thumbnails into a sealed payload. With no thumbnails the
/// bytes are the ones older versions wrote.
pub(crate) fn pack_full(
    text: &str,
    files: &[Attached],
    embeds: &[crate::previews::Embed],
    thumbs: &[Thumb],
) -> Result<String> {
    pack_all(text, files, embeds, thumbs, None)
}

/// Packs everything a message can carry. `mentions` is left out when it names nobody.
pub(crate) fn pack_all(
    text: &str,
    files: &[Attached],
    embeds: &[crate::previews::Embed],
    thumbs: &[Thumb],
    mentions: Option<&Mentions>,
) -> Result<String> {
    pack_all_with(text, files, embeds, thumbs, mentions, &[])
}

/// Packs everything a message can carry, with the pictures and theme colours of its link previews.
pub(crate) fn pack_all_with(
    text: &str,
    files: &[Attached],
    embeds: &[crate::previews::Embed],
    thumbs: &[Thumb],
    mentions: Option<&Mentions>,
    extras: &[EmbedExtra],
) -> Result<String> {
    let mut bytes = postcard::to_allocvec(&Packed {
        text: text.to_string(),
        files: files.to_vec(),
        embeds: embeds.to_vec(),
    })
    .map_err(|e| CoreError::Encoding(e.to_string()))?;
    let mut exts = Vec::new();
    if !thumbs.is_empty() {
        let body = postcard::to_allocvec(thumbs).map_err(|e| CoreError::Encoding(e.to_string()))?;
        exts.push((EXT_THUMBS, body));
    }
    if let Some(m) = mentions.filter(|m| !m.is_empty()) {
        let body = postcard::to_allocvec(m).map_err(|e| CoreError::Encoding(e.to_string()))?;
        exts.push((EXT_MENTIONS, body));
    }
    if !extras.is_empty() {
        let body = postcard::to_allocvec(extras).map_err(|e| CoreError::Encoding(e.to_string()))?;
        exts.push((EXT_EMBEDS, body));
    }
    if !exts.is_empty() {
        let trailer = postcard::to_allocvec(&Trailer { exts }).map_err(|e| CoreError::Encoding(e.to_string()))?;
        bytes.extend_from_slice(&trailer);
    }
    Ok(format!("{MARK}{}", data_encoding::BASE64URL_NOPAD.encode(&bytes)))
}

/// The parts of a sealed payload.
struct Opened {
    text: String,
    files: Vec<Attached>,
    embeds: Vec<crate::previews::Embed>,
    thumbs: Vec<Thumb>,
    mentions: Mentions,
    extras: Vec<EmbedExtra>,
}

/// Opens a sealed payload. Plain text has none.
fn open(plain: &str) -> Option<Opened> {
    let body = plain.strip_prefix(MARK)?;
    let bytes = data_encoding::BASE64URL_NOPAD.decode(body.as_bytes()).ok()?;
    let (packed, rest) = postcard::take_from_bytes::<Packed>(&bytes).ok()?;
    // Whatever follows `Packed` is the extensions. A trailer that does not decode is ignored.
    let thumbs = if rest.is_empty() { Vec::new() } else { trailer_thumbs(rest) };
    let mentions = if rest.is_empty() { Mentions::default() } else { trailer_mentions(rest) };
    let extras = if rest.is_empty() {
        Vec::new()
    } else {
        trailer_body::<Vec<EmbedExtra>>(rest, EXT_EMBEDS).unwrap_or_default()
    };
    Some(Opened {
        text: packed.text,
        files: packed.files,
        embeds: packed.embeds,
        thumbs,
        mentions,
        extras,
    })
}

/// The body of the first extension with `tag` that decodes as `T`. Tags other than `tag` are
/// skipped, and so is a body that does not decode. A trailer that does not decode gives none.
fn trailer_body<T: serde::de::DeserializeOwned>(rest: &[u8], tag: u8) -> Option<T> {
    let trailer = postcard::from_bytes::<Trailer>(rest).ok()?;
    trailer
        .exts
        .iter()
        .filter(|(t, _)| *t == tag)
        .find_map(|(_, body)| postcard::from_bytes::<T>(body).ok())
}

/// The thumbnails in a trailer.
fn trailer_thumbs(rest: &[u8]) -> Vec<Thumb> {
    trailer_body::<Vec<Thumb>>(rest, EXT_THUMBS).unwrap_or_default()
}

/// The mentions in a trailer.
fn trailer_mentions(rest: &[u8]) -> Mentions {
    trailer_body::<Mentions>(rest, EXT_MENTIONS).unwrap_or_default()
}

/// Who a sealed payload mentions, as the sender wrote it. Nothing is checked here: see the caller.
pub(crate) fn mentions_of(plain: &str) -> Mentions {
    open(plain).map(|o| o.mentions).unwrap_or_default()
}

/// Splits a sealed payload into text and files. Plain text has no files.
pub(crate) fn unpack(plain: &str) -> (String, Vec<Attached>) {
    if !plain.starts_with(MARK) {
        return (plain.to_string(), Vec::new());
    }
    open(plain).map_or((String::new(), Vec::new()), |o| (o.text, o.files))
}

/// The link previews in a sealed payload.
pub(crate) fn embeds_of(plain: &str) -> Vec<crate::previews::Embed> {
    open(plain).map(|o| o.embeds).unwrap_or_default()
}

/// The pictures and theme colours of a sealed payload's link previews. An entry that names no preview,
/// or whose picture this device would not fetch, is left out.
pub(crate) fn embed_extras_of(plain: &str) -> Vec<EmbedExtra> {
    let Some(opened) = open(plain) else {
        return Vec::new();
    };
    let previews = opened.embeds.len();
    opened
        .extras
        .into_iter()
        .filter(|e| (e.index as usize) < previews)
        .filter(|e| match &e.image {
            Some(image) => valid_picture(image),
            None => true,
        })
        .collect()
}

/// A link preview's picture is trusted only when it is small enough to fetch on arrival and its size
/// is one a preview picture can have. Anything else is ignored.
fn valid_picture(p: &EmbedImage) -> bool {
    p.file.size <= PREVIEW_IMAGE_MAX_BYTES as u64
        && p.file.check(PREVIEW_IMAGE_MAX_BYTES as u64).is_ok()
        && (1..=PREVIEW_IMAGE_SIDE).contains(&p.width)
        && (1..=PREVIEW_IMAGE_SIDE).contains(&p.height)
}

/// The link previews of a sealed payload as the UI shows them, with the picture and theme colour this
/// device has. A picture still on its way is left out until it arrives.
pub(crate) fn embed_views(paths: &crate::store::Paths, plain: &str) -> Vec<crate::previews::EmbedView> {
    let extras = embed_extras_of(plain);
    embeds_of(plain)
        .into_iter()
        .enumerate()
        .map(|(i, embed)| {
            let extra = extras.iter().find(|e| e.index as usize == i);
            let picture = extra
                .and_then(|e| e.image.as_ref())
                .filter(|img| thumb_path(paths, &attachment_id(&img.file)).is_file());
            crate::previews::EmbedView {
                embed,
                image: picture.map(|img| thumb_path(paths, &attachment_id(&img.file)).to_string_lossy().into_owned()),
                image_w: picture.map(|img| img.width),
                image_h: picture.map(|img| img.height),
                theme: extra
                    .and_then(|e| e.theme)
                    .map(|c| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])),
            }
        })
        .collect()
}

/// The thumbnails in a sealed payload. A thumbnail that could not be fetched on arrival is left out.
pub(crate) fn thumbs_of(plain: &str) -> Vec<Thumb> {
    open(plain)
        .map(|o| o.thumbs.into_iter().filter(valid_thumb).collect())
        .unwrap_or_default()
}

/// A sender's thumbnail is trusted only when it is small enough to fetch on arrival and its size is
/// one a thumbnail can have. Anything else is ignored, so no message makes this device fetch more
/// than `THUMB_AUTO_BYTES`, whatever the sender writes.
fn valid_thumb(t: &Thumb) -> bool {
    t.file.size <= THUMB_AUTO_BYTES
        && t.file.check(THUMB_AUTO_BYTES).is_ok()
        && (1..=THUMB_LONG_SIDE).contains(&t.width)
        && (1..=THUMB_LONG_SIDE).contains(&t.height)
}

/// The text part of a sealed payload.
pub(crate) fn text_of(plain: &str) -> String {
    unpack(plain).0
}

/// The id shown for an attachment: the base32 of the ciphertext hash.
pub(crate) fn attachment_id(file: &FileRef) -> String {
    data_encoding::BASE32_NOPAD.encode(&file.hash).to_lowercase()
}

/// A file as the UI shows it. Field names follow `07-visual-system.md`, Part 5.
#[derive(Clone, Debug, serde::Serialize)]
pub struct AttachmentView {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub mime: String,
    /// `available`, `downloading`, `paused`, `downloaded`, `failed`, `disk_full` or `too_large`.
    pub state: String,
    /// `0..1`.
    pub progress: f64,
    pub local_path: Option<String>,
    /// True until the file is on this device.
    pub needed: bool,
    pub seed: bool,
    /// The preview, for an image that has one.
    pub thumb: Option<ThumbView>,
    /// The download's id, while this device has a transfer for the file.
    pub transfer_id: Option<String>,
    /// The providers that delivered bytes to the download, in the order they were tried.
    pub sources: Vec<FileSourceView>,
    /// Verified pieces of the download, while it has any.
    pub pieces: Option<PieceCounts>,
}

/// One provider of a download, as the attachment card and the details panel show it.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct FileSourceView {
    pub display_name: String,
    /// `direct`. The file store does not say whether a provider was reached directly or by relay.
    pub route: String,
    /// The average rate of this provider over the transfer so far, in bytes per second.
    pub rate_bps: u64,
}

/// Verified pieces of a download. A piece is 64 KiB of ciphertext, checked as it arrives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct PieceCounts {
    pub total: u64,
    pub verified: u64,
}

/// The live per-provider counts of one download. Kept in memory: a restart starts the counts again.
#[derive(Clone, Debug, Default)]
pub(crate) struct LiveSources {
    /// The providers the download was given, in the order the file store tries them.
    pub providers: Vec<PeerAddr>,
    pub counts: Vec<SourceCount>,
    pub cipher: u64,
}

/// The preview of an image. `state` is `available` until the thumbnail is fetched, then
/// `downloading` and `downloaded`. `path` is set once it is on this device.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ThumbView {
    pub state: String,
    pub path: Option<String>,
    pub width: u32,
    pub height: u32,
}

/// Thumbnails at or under this size are fetched when their message arrives.
pub(crate) const THUMB_AUTO_BYTES: u64 = 64 * 1024;

/// A download or upload, as `list_transfers` lists it. `state` is `active`, `paused`, `waiting`,
/// `done` or `failed`. `direction` is `download` or `upload`.
#[derive(Clone, Debug, serde::Serialize)]
pub struct TransferView {
    pub id: String,
    pub message_id: String,
    pub direction: String,
    pub name: String,
    pub size: u64,
    pub state: String,
    pub progress: f64,
    pub peers: usize,
    pub started_ms: u64,
    pub local_path: Option<String>,
    pub error: Option<String>,
    /// The providers that have been tried or can be used, with what each delivered.
    pub sources: Vec<SourceView>,
    /// Verified pieces, while the download has counts.
    pub pieces: Option<PieceCounts>,
}

/// One provider of a transfer.
#[derive(Clone, Debug, serde::Serialize)]
pub struct SourceView {
    pub device: String,
    pub name: String,
    /// `waiting` while the transfer runs, and `done` once it finishes.
    pub state: String,
    /// Ciphertext bytes this provider delivered that are in the store.
    pub bytes: u64,
    /// Pieces this provider's bytes completed, each checked against the file's hashes.
    pub verified_pieces: u64,
}

/// One transfer the engine tracks. Downloads keep their providers, so they can resume.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Transfer {
    pub id: String,
    pub message_id: String,
    pub direction: String,
    pub file: FileRef,
    /// Provider addresses, as `PeerAddr` strings.
    pub providers: Vec<String>,
    pub state: String,
    pub progress: f64,
    pub started_ms: u64,
    pub local_path: Option<PathBuf>,
    pub error: Option<String>,
}

/// Attachment state that survives restarts, plus the transfers of this run.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct AttachBook {
    /// Mirror limits for seeding, as `(max_file_size, max_total)`. `None` means this device does
    /// not mirror.
    pub mirror: Option<(u64, u64)>,
    /// Files at or under this size download on arrival. `None` means always ask.
    pub auto_accept_bytes: Option<u64>,
    /// Whether this device makes link previews for messages it sends. Off by default.
    pub link_previews: bool,
    /// Where finished downloads go. `None` means the default folder, which follows the OS Downloads folder.
    pub download_dir: Option<PathBuf>,
    /// The folder downloads use when none is picked: the OS Downloads folder, set by the app on each start. Not
    /// stored, so a moved OS folder is picked up on the next start.
    #[serde(skip)]
    pub default_download_dir: Option<PathBuf>,
    /// Finished downloads: attachment id to local path.
    pub downloads: BTreeMap<String, PathBuf>,
    pub transfers: BTreeMap<String, Transfer>,
    /// Mirrored ciphertext: attachment id to its size.
    pub mirrored: BTreeMap<String, u64>,
    /// Files whose mirror is running now.
    #[serde(skip)]
    pub mirroring: std::collections::BTreeSet<String>,
    /// Running download tasks, by attachment id.
    #[serde(skip)]
    pub tasks: BTreeMap<String, tokio::task::JoinHandle<()>>,
}

impl AttachBook {
    pub fn load(paths: &crate::store::Paths) -> Result<Self> {
        let mut book: AttachBook = crate::store::read_bin(&paths.attachments())?.unwrap_or_default();
        // A download that was running when the engine stopped is paused. Its bytes are kept.
        for t in book.transfers.values_mut() {
            if t.state == "active" || t.state == "waiting" {
                t.state = "paused".into();
            }
        }
        Ok(book)
    }

    pub fn save(&self, paths: &crate::store::Paths) -> Result<()> {
        crate::store::write_bin(&paths.attachments(), self)
    }
}

fn mime_of(path: &Path) -> String {
    mime_guess::from_path(path)
        .first_or_octet_stream()
        .essence_str()
        .to_string()
}

impl World {
    /// The attachments of a message, with their state on this device.
    pub(crate) fn attachment_views(&self, text: &str) -> Vec<AttachmentView> {
        let (_, files) = unpack(text);
        let thumbs = thumbs_of(text);
        files
            .iter()
            .map(|a| {
                let id = attachment_id(&a.file);
                let thumb = thumbs
                    .iter()
                    .find(|t| t.of == a.file.hash)
                    .map(|t| self.thumb_view(&id, t));
                let (state, progress, local_path) = if let Some(path) = self.attach.downloads.get(&id) {
                    (
                        "downloaded".to_string(),
                        1.0,
                        Some(path.to_string_lossy().into_owned()),
                    )
                } else if let Some(t) = self.attach.transfers.get(&id) {
                    let state = match t.state.as_str() {
                        "active" | "waiting" => "downloading",
                        "paused" => "paused",
                        _ => "failed",
                    };
                    (state.to_string(), t.progress, None)
                } else if a.file.size > MAX_FILE_SIZE {
                    ("too_large".to_string(), 0.0, None)
                } else {
                    ("available".to_string(), 0.0, None)
                };
                let live = self.source_counts.get(&id);
                let transfer = self.attach.transfers.get(&id);
                let elapsed_ms = transfer.map_or(0, |t| now_ms().saturating_sub(t.started_ms));
                let sources = live
                    .map(|l| {
                        file_sources(
                            &source_views(l, "waiting", |p| device_name(self, p)),
                            elapsed_ms,
                        )
                    })
                    .unwrap_or_default();
                AttachmentView {
                    id: id.clone(),
                    name: a.file.safe_name(),
                    size: a.file.size,
                    mime: a.file.mime.clone(),
                    needed: state != "downloaded",
                    state,
                    progress,
                    local_path,
                    seed: a.seed,
                    thumb,
                    transfer_id: transfer.map(|_| id),
                    sources,
                    pieces: live.map(piece_counts),
                }
            })
            .collect()
    }

    /// The preview of an attachment on this device.
    fn thumb_view(&self, id: &str, t: &Thumb) -> ThumbView {
        let path = thumb_path(&self.paths, id);
        let (state, path) = if path.is_file() {
            ("downloaded", Some(path.to_string_lossy().into_owned()))
        } else if self.thumb_fetching.contains(id) {
            ("downloading", None)
        } else {
            ("available", None)
        };
        ThumbView {
            state: state.into(),
            path,
            width: t.width,
            height: t.height,
        }
    }
}

/// The blobs a message's thumbnails and preview pictures point at, as `(hash, ciphertext size)`. Seeders are told these
/// so they can keep them. See `mirror_hint`.
pub(crate) fn media_blobs(plain: &str) -> Vec<([u8; 32], u64)> {
    let Some(o) = open(plain) else {
        return Vec::new();
    };
    let thumbs = o.thumbs.iter().filter(|t| valid_thumb(t)).map(|t| (t.file.hash, t.file.cipher_size));
    let previews = o.extras.iter().filter_map(|e| e.image.as_ref()).map(|i| (i.file.hash, i.file.cipher_size));
    thumbs.chain(previews).collect()
}

/// Where the thumbnail of an attachment is stored on this device.
fn thumb_path(paths: &crate::store::Paths, id: &str) -> PathBuf {
    paths.thumbnails_dir().join(format!("{id}.jpg"))
}

impl Engine {
    /// Encrypts the files and returns them ready to pack into a message, with the thumbnails of the
    /// images among them.
    pub(crate) async fn add_files(&self, paths: &[PathBuf], seed: bool) -> Result<(Vec<Attached>, Vec<Thumb>)> {
        let mut out = Vec::new();
        let mut thumbs = Vec::new();
        for path in paths {
            let mime = mime_of(path);
            let file = self
                .inner
                .files
                .add(Input::Path {
                    path: path.clone(),
                    mime,
                })
                .await
                .map_err(files_err)?;
            self.record_upload(&file);
            if let Some(thumb) = self.thumbnail_of(path, &file).await {
                thumbs.push(thumb);
            }
            out.push(Attached {
                file,
                from: self.inner.me,
                seed,
            });
        }
        Ok((out, thumbs))
    }

    /// Makes and stores the thumbnail of an image file. Other files have none, and neither does an
    /// image that cannot be decoded or is too large to preview.
    async fn thumbnail_of(&self, path: &Path, file: &FileRef) -> Option<Thumb> {
        if !file.mime.starts_with("image/") {
            return None;
        }
        let len = tokio::fs::metadata(path).await.ok()?.len();
        if len > THUMB_SOURCE_MAX as u64 {
            return None;
        }
        let data = tokio::fs::read(path).await.ok()?;
        let Thumbnail { jpeg, width, height } = tokio::task::spawn_blocking(move || make_thumbnail(&data))
            .await
            .ok()??;
        let stored = self
            .inner
            .files
            .add(Input::Bytes {
                name: "thumbnail.jpg".into(),
                mime: "image/jpeg".into(),
                data: jpeg,
            })
            .await
            .map_err(|e| debug!(error = %e, "could not store a thumbnail"))
            .ok()?;
        Some(Thumb {
            of: file.hash,
            file: stored,
            width,
            height,
        })
    }

    fn record_upload(&self, file: &FileRef) {
        let mut guard = self.write();
        let id = attachment_id(file);
        guard.attach.transfers.insert(
            id.clone(),
            Transfer {
                id,
                message_id: String::new(),
                direction: "upload".into(),
                file: file.clone(),
                providers: Vec::new(),
                state: "done".into(),
                progress: 1.0,
                started_ms: now_ms(),
                local_path: None,
                error: None,
            },
        );
    }

    /// Sends a message with files in a channel. `text` may be empty.
    pub async fn send_files(
        &self,
        community: dissent_identity::CommunityId,
        channel: dissent_events::ChannelId,
        paths: Vec<PathBuf>,
        text: String,
        seed: bool,
    ) -> Result<crate::views::MessageItem> {
        self.send_files_with(community, channel, paths, text, seed, Mentions::default()).await
    }

    /// Sends files to a channel with a message that mentions the people in `mentions`.
    pub async fn send_files_with(
        &self,
        community: dissent_identity::CommunityId,
        channel: dissent_events::ChannelId,
        paths: Vec<PathBuf>,
        text: String,
        seed: bool,
        mentions: Mentions,
    ) -> Result<crate::views::MessageItem> {
        let (attached, thumbs) = self.add_files(&paths, seed).await?;
        let plain = pack_all(text.trim(), &attached, &[], &thumbs, Some(&mentions))?;
        self.send_channel_plain(community, channel, plain).await
    }

    /// Sends files to a conversation. `text` may be empty.
    pub async fn send_conversation_files(
        &self,
        conv: crate::groups::ConvId,
        paths: Vec<PathBuf>,
        text: String,
    ) -> Result<crate::views::DmItem> {
        let (attached, thumbs) = self.add_files(&paths, true).await?;
        let plain = pack_full(text.trim(), &attached, &[], &thumbs)?;
        self.send_conversation_plain(conv, plain).await
    }

    /// Sends files to a person in a 1:1 conversation. `text` may be empty.
    pub async fn send_dm_files(
        &self,
        peer: dissent_identity::RootId,
        paths: Vec<PathBuf>,
        text: String,
    ) -> Result<crate::views::DmItem> {
        let conv = self.ensure_dm(peer)?;
        self.send_conversation_files(conv, paths, text).await
    }

    /// Providers for files from `roots`: their devices whose addresses we know.
    pub(crate) fn providers_for(&self, roots: &[RootId]) -> Vec<PeerAddr> {
        let w = self.read();
        let devices: Vec<dissent_identity::DeviceId> =
            roots.iter().flat_map(|r| w.directory.devices_of(r)).collect();
        w.addresses
            .peers()
            .into_iter()
            .filter(|p| devices.contains(&p.id) && p.id != self.inner.identity.device_id())
            .collect()
    }

    /// Starts a download for one attachment of a message. Community messages are found through
    /// `community`, and direct messages through `None`.
    pub async fn download_attachment(
        &self,
        community: Option<dissent_identity::CommunityId>,
        message: String,
        attachment: String,
    ) -> Result<TransferView> {
        let (file, roots, message_id) = self.locate_attachment(community, &message, &attachment)?;
        let id = attachment_id(&file);
        if self.read().attach.downloads.contains_key(&id) {
            return self.transfer_view(&id);
        }
        if file.size > MAX_FILE_SIZE {
            return Err(CoreError::Invalid("file is over the size limit".into()));
        }
        let providers = self.providers_for(&roots);
        {
            let mut guard = self.write();
            if let Some(t) = guard.attach.transfers.get(&id) {
                if t.state == "active" || t.state == "waiting" {
                    drop(guard);
                    return self.transfer_view(&id);
                }
            }
            guard.attach.transfers.insert(
                id.clone(),
                Transfer {
                    id: id.clone(),
                    message_id: message_id.clone(),
                    direction: "download".into(),
                    file,
                    providers: providers.iter().map(|p| p.to_string()).collect(),
                    state: "active".into(),
                    progress: 0.0,
                    started_ms: now_ms(),
                    local_path: None,
                    error: None,
                },
            );
        }
        self.persist_attach();
        self.start_download(&id);
        self.emit(EngineEvent::TransfersChanged);
        self.transfer_view(&id)
    }

    /// The sealed text of a message, and the roots that may hold its files.
    fn message_text(
        &self,
        community: Option<dissent_identity::CommunityId>,
        message: &str,
    ) -> Result<(String, Vec<RootId>)> {
        let w = self.read();
        match community {
            Some(c) => {
                let comm = w
                    .communities
                    .get(&c)
                    .ok_or_else(|| CoreError::NotFound(format!("community {c}")))?;
                let id: dissent_events::EventId = message
                    .parse()
                    .map_err(|_| CoreError::Invalid("not a message id".into()))?;
                let text = comm
                    .content
                    .get(&id)
                    .map(|(_, t)| t.clone())
                    .ok_or_else(|| CoreError::NotFound(format!("message {message}")))?;
                let roots: Vec<RootId> = comm.state().members.keys().copied().collect();
                Ok((text, roots))
            }
            None => {
                let m = w
                    .conv_msgs
                    .values()
                    .flatten()
                    .find(|m| {
                        data_encoding::BASE32_NOPAD.encode(&m.id).to_lowercase() == message
                    })
                    .ok_or_else(|| CoreError::NotFound(format!("message {message}")))?;
                let roots = w
                    .conv_book
                    .convs
                    .get(&m.conv)
                    .map(|meta| meta.members.clone())
                    .unwrap_or_default();
                Ok((m.text.clone(), roots))
            }
        }
    }

    /// The file, providers and message id for an attachment, found in the message text.
    fn locate_attachment(
        &self,
        community: Option<dissent_identity::CommunityId>,
        message: &str,
        attachment: &str,
    ) -> Result<(FileRef, Vec<RootId>, String)> {
        let (text, mut roots) = self.message_text(community, message)?;
        let (_, files) = unpack(&text);
        let found = files
            .into_iter()
            .find(|a| attachment_id(&a.file) == attachment)
            .ok_or_else(|| CoreError::NotFound(format!("attachment {attachment}")))?;
        roots.retain(|r| *r != self.inner.me);
        if !roots.contains(&found.from) {
            roots.insert(0, found.from);
        }
        Ok((found.file, roots, message.to_string()))
    }

    /// The thumbnail of an attachment, and the providers that may hold it. The sender's roots come
    /// first, as they do for the file.
    fn locate_thumb(
        &self,
        community: Option<dissent_identity::CommunityId>,
        message: &str,
        attachment: &str,
    ) -> Result<(FileRef, Vec<RootId>)> {
        let (text, mut roots) = self.message_text(community, message)?;
        let (_, files) = unpack(&text);
        let of = files
            .iter()
            .find(|a| attachment_id(&a.file) == attachment)
            .ok_or_else(|| CoreError::NotFound(format!("attachment {attachment}")))?;
        let thumb = thumbs_of(&text)
            .into_iter()
            .find(|t| t.of == of.file.hash)
            .ok_or_else(|| CoreError::NotFound(format!("thumbnail of {attachment}")))?;
        roots.retain(|r| *r != self.inner.me);
        if !roots.contains(&of.from) {
            roots.insert(0, of.from);
        }
        Ok((thumb.file, roots))
    }

    /// The picture of a link preview, and the providers that may hold it, found by its attachment id.
    fn locate_embed_image(
        &self,
        community: Option<dissent_identity::CommunityId>,
        message: &str,
        picture: &str,
    ) -> Result<(FileRef, Vec<RootId>)> {
        let (text, mut roots) = self.message_text(community, message)?;
        let image = embed_extras_of(&text)
            .into_iter()
            .filter_map(|e| e.image)
            .find(|img| attachment_id(&img.file) == picture)
            .ok_or_else(|| CoreError::NotFound(format!("picture {picture}")))?;
        roots.retain(|r| *r != self.inner.me);
        Ok((image.file, roots))
    }

    fn transfer_view(&self, id: &str) -> Result<TransferView> {
        let w = self.read();
        if let Some(path) = w.attach.downloads.get(id) {
            return Ok(TransferView {
                id: id.to_string(),
                message_id: String::new(),
                direction: "download".into(),
                name: String::new(),
                size: 0,
                state: "done".into(),
                progress: 1.0,
                peers: 0,
                started_ms: 0,
                local_path: Some(path.to_string_lossy().into_owned()),
                error: None,
                sources: Vec::new(),
                pieces: None,
            });
        }
        let t = w
            .attach
            .transfers
            .get(id)
            .ok_or_else(|| CoreError::NotFound(format!("transfer {id}")))?;
        Ok(view_of(t, &w))
    }

    /// Every transfer this run, downloads and uploads.
    pub fn transfers(&self) -> Vec<TransferView> {
        let w = self.read();
        w.attach.transfers.values().map(|t| view_of(t, &w)).collect()
    }

    /// Stops a download and keeps what it has. It can resume later.
    pub fn pause_transfer(&self, id: &str) -> Result<()> {
        let running = self.read().attach.transfers.get(id).is_some_and(|t| t.state == "active");
        if !running {
            return Err(CoreError::Invalid("transfer is not running".into()));
        }
        self.abort_task(id);
        self.set_state(id, "paused", None)
    }

    /// Writes the attachment book to disk.
    fn persist_attach(&self) {
        let guard = self.read();
        if let Err(e) = guard.attach.save(&guard.paths) {
            warn!(error = %e, "could not save transfers");
        }
    }

    /// Mirrored attachments on this device, by attachment id.
    pub fn mirrored_attachments(&self) -> Vec<String> {
        self.read().attach.mirrored.keys().cloned().collect()
    }

    /// Continues a paused or failed download from the bytes it already has.
    pub fn resume_transfer(&self, id: &str) -> Result<()> {
        self.set_state(id, "active", None)?;
        self.start_download(id);
        Ok(())
    }

    pub fn retry_transfer(&self, id: &str) -> Result<()> {
        self.resume_transfer(id)
    }

    /// Stops a download and drops its row. The partial bytes stay until they expire.
    pub fn cancel_transfer(&self, id: &str) -> Result<()> {
        self.abort_task(id);
        self.write().attach.transfers.remove(id);
        self.persist_attach();
        self.emit(EngineEvent::TransfersChanged);
        Ok(())
    }

    /// Removes a row from the list. Same as cancel for a finished transfer.
    pub fn remove_transfer(&self, id: &str) -> Result<()> {
        self.write().source_counts.remove(id);
        self.cancel_transfer(id)
    }

    /// Stores the latest per-provider counts of a download, for the views.
    fn record_sources(&self, id: &str, counts: &[SourceCount]) {
        if let Some(live) = self.write().source_counts.get_mut(id) {
            live.counts = counts.to_vec();
        }
    }

    fn abort_task(&self, id: &str) {
        if let Some(h) = self.write().attach.tasks.remove(id) {
            h.abort();
        }
    }

    fn set_state(&self, id: &str, state: &str, error: Option<String>) -> Result<()> {
        let mut guard = self.write();
        let t = guard
            .attach
            .transfers
            .get_mut(id)
            .ok_or_else(|| CoreError::NotFound(format!("transfer {id}")))?;
        t.state = state.to_string();
        if error.is_some() {
            t.error = error;
        }
        drop(guard);
        self.emit(EngineEvent::TransfersChanged);
        Ok(())
    }

    /// Spawns the task that fetches one download, and keeps its handle so it can be paused.
    fn start_download(&self, id: &str) {
        let engine = Engine {
            inner: self.inner.clone(),
        };
        let id_owned = id.to_string();
        let handle = tokio::spawn(async move {
            engine.run_download(&id_owned).await;
        });
        self.write().attach.tasks.insert(id.to_string(), handle);
    }

    async fn run_download(&self, id: &str) {
        let (file, providers, message_id) = {
            let w = self.read();
            match w.attach.transfers.get(id) {
                Some(t) => (t.file.clone(), t.providers.clone(), t.message_id.clone()),
                None => return,
            }
        };
        let (peers, from): (Vec<PeerAddr>, Vec<_>) = providers
            .iter()
            .filter_map(|p| p.parse::<PeerAddr>().ok())
            .filter_map(|p| p.clone().to_iroh().ok().map(|iroh| (p, iroh)))
            .unzip();
        self.write().source_counts.insert(
            id.to_string(),
            LiveSources {
                providers: peers,
                counts: Vec::new(),
                cipher: file.cipher_size,
            },
        );
        let node = self.node();
        let dest = self.download_dir();
        if let Err(e) = std::fs::create_dir_all(&dest) {
            self.fail(id, "failed", format!("cannot create downloads: {e}"));
            return;
        }
        let engine = Engine {
            inner: self.inner.clone(),
        };
        let att = id.to_string();
        let msg = message_id.clone();
        let sink = Engine {
            inner: self.inner.clone(),
        };
        let sink_id = id.to_string();
        let result = self
            .inner
            .files
            .fetch_tallied(
                node.endpoint(),
                &from,
                &file,
                &dest,
                move |held, total| {
                    let progress = if total == 0 { 0.0 } else { held as f64 / total as f64 };
                    engine.set_progress(&att, progress, &msg);
                },
                move |counts| sink.record_sources(&sink_id, counts),
            )
            .await;
        match result {
            Ok(path) => {
                {
                    let mut guard = self.write();
                    guard.attach.downloads.insert(id.to_string(), path.clone());
                    if let Some(t) = guard.attach.transfers.get_mut(id) {
                        t.state = "done".into();
                        t.progress = 1.0;
                        t.local_path = Some(path.clone());
                    }
                    guard.attach.tasks.remove(id);
                    if let Err(e) = guard.attach.save(&guard.paths) {
                        warn!(error = %e, "could not save download state");
                    }
                }
                self.emit(EngineEvent::AttachmentProgress {
                    message_id,
                    attachment_id: id.to_string(),
                    progress: 1.0,
                });
                self.emit(EngineEvent::TransfersChanged);
            }
            Err(FilesError::DiskFull { .. }) => self.fail(id, "disk_full", "not enough disk space".into()),
            Err(e) => {
                debug!(error = %e, "download failed");
                self.fail(id, "failed", e.to_string());
            }
        }
    }

    fn fail(&self, id: &str, state: &str, error: String) {
        {
            let mut guard = self.write();
            guard.attach.tasks.remove(id);
            if let Some(t) = guard.attach.transfers.get_mut(id) {
                t.state = "failed".into();
                t.error = Some(format!("{state}: {error}"));
            }
        }
        self.persist_attach();
        self.emit(EngineEvent::TransfersChanged);
    }

    fn set_progress(&self, id: &str, progress: f64, message_id: &str) {
        let changed = {
            let mut guard = self.write();
            match guard.attach.transfers.get_mut(id) {
                Some(t) => {
                    t.progress = progress;
                    true
                }
                None => false,
            }
        };
        if changed {
            self.emit(EngineEvent::AttachmentProgress {
                message_id: message_id.to_string(),
                attachment_id: id.to_string(),
                progress,
            });
        }
    }

    /// Turns on mirroring for seeding, or turns it off with `None`.
    pub fn set_mirror_policy(&self, policy: Option<MirrorPolicy>) -> Result<()> {
        let mut guard = self.write();
        guard.attach.mirror = policy.map(|p| (p.max_file_size, p.max_total));
        guard.attach.save(&guard.paths)
    }

    /// Called when a message with files arrives. A device that opted in mirrors the ciphertext of
    /// every file marked `seed`, within its policy. Mirroring runs in the background.
    pub(crate) fn maybe_mirror(&self, plain: &str) {
        let (_, files) = unpack(plain);
        if files.is_empty() {
            return;
        }
        let policy = {
            let w = self.read();
            match w.attach.mirror {
                Some((max_file_size, max_total)) => MirrorPolicy {
                    max_file_size,
                    max_total,
                },
                None => return,
            }
        };
        for a in files.into_iter().filter(|a| a.seed && a.from != self.inner.me) {
            let providers: Vec<_> = self
                .providers_for(&[a.from])
                .iter()
                .filter_map(|p| p.to_iroh().ok())
                .collect();
            let engine = Engine {
                inner: self.inner.clone(),
            };
            let policy = MirrorPolicy {
                max_file_size: policy.max_file_size,
                max_total: policy.max_total,
            };
            let id = attachment_id(&a.file);
            let size = a.file.cipher_size;
            tokio::spawn(async move {
                let node = engine.node();
                let result = engine
                    .inner
                    .files
                    .mirror(
                        node.endpoint(),
                        &providers,
                        a.file.hash,
                        a.file.cipher_size,
                        policy,
                        |_, _| {},
                    )
                    .await;
                match result {
                    Ok(()) => {
                        let mut guard = engine.write();
                        guard.attach.mirrored.insert(id, size);
                        if let Err(e) = guard.attach.save(&guard.paths) {
                            warn!(error = %e, "could not save mirror state");
                        }
                    }
                    Err(e) => debug!(error = %e, "not mirroring a file"),
                }
            });
        }
    }

    /// Opens the Files store. Mounted on the node, so it lives as long as the node.
    pub(crate) async fn open_files(dir: &Path) -> Result<dissent_files::Files> {
        dissent_files::Files::open(dir)
            .await
            .map_err(files_err)
    }
}

/// The name shown for a provider device: its person's name, or the device id when it is unknown.
fn device_name(w: &World, p: &PeerAddr) -> String {
    w.directory
        .root_of(&p.id)
        .map(|r| w.display_name(&r))
        .unwrap_or_else(|| p.id.to_string())
}

/// The per-provider view of a download's live counts, in the order the providers were tried.
pub(crate) fn source_views(
    live: &LiveSources,
    state: &str,
    name_of: impl Fn(&PeerAddr) -> String,
) -> Vec<SourceView> {
    live.providers
        .iter()
        .zip(live.counts.iter())
        .map(|(p, c)| SourceView {
            device: p.id.to_string(),
            name: name_of(p),
            state: state.to_string(),
            bytes: c.bytes,
            verified_pieces: c.pieces,
        })
        .collect()
}

/// The pieces of a download, from its live counts.
pub(crate) fn piece_counts(live: &LiveSources) -> PieceCounts {
    PieceCounts {
        total: pieces_of(live.cipher),
        verified: live.counts.iter().map(|c| c.pieces).sum(),
    }
}

/// The providers that delivered bytes, as the attachment card lists them. A provider that has not
/// delivered anything yet is left out, so "From N sources" counts the ones that served the file.
pub(crate) fn file_sources(sources: &[SourceView], elapsed_ms: u64) -> Vec<FileSourceView> {
    sources
        .iter()
        .filter(|s| s.bytes > 0)
        .map(|s| FileSourceView {
            display_name: s.name.clone(),
            route: "direct".into(),
            rate_bps: s.bytes.saturating_mul(1000).checked_div(elapsed_ms).unwrap_or(0),
        })
        .collect()
}

fn view_of(t: &Transfer, w: &World) -> TransferView {
    let live = w.source_counts.get(&t.id);
    let sources = match live {
        Some(live) => {
            let state = if t.state == "done" { "done" } else { "waiting" };
            source_views(live, state, |p| device_name(w, p))
        }
        None => t
            .providers
            .iter()
            .filter_map(|p| p.parse::<PeerAddr>().ok())
            .map(|p| SourceView {
                device: p.id.to_string(),
                name: device_name(w, &p),
                state: if t.state == "done" { "done" } else { "waiting" }.to_string(),
                bytes: 0,
                verified_pieces: 0,
            })
            .collect(),
    };
    TransferView {
        pieces: live.map(piece_counts),
        sources,
        id: t.id.clone(),
        message_id: t.message_id.clone(),
        direction: t.direction.clone(),
        name: t.file.safe_name(),
        size: t.file.size,
        state: t.state.clone(),
        progress: t.progress,
        peers: t.providers.len(),
        started_ms: t.started_ms,
        local_path: t.local_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
        error: t.error.clone(),
    }
}

fn files_err(e: FilesError) -> CoreError {
    CoreError::Invalid(format!("files: {e}"))
}

/// Starts a cleartext manifest in front of a sealed payload. The manifest lists each file's
/// ciphertext hash and size, and whether it may be mirrored. It is inside the signed event, so
/// it cannot be changed. No key, name or MIME type is in it.
pub(crate) const MANIFEST_MAGIC: &[u8] = b"DFM1";

/// One file in the cleartext manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ManifestEntry {
    pub hash: [u8; 32],
    pub cipher_size: u64,
    pub seed: bool,
}

/// The manifest for the files in a sealed text.
pub(crate) fn manifest_of(plain: &str) -> Vec<ManifestEntry> {
    unpack(plain)
        .1
        .iter()
        .map(|a| ManifestEntry {
            hash: a.file.hash,
            cipher_size: a.file.cipher_size,
            seed: a.seed,
        })
        .collect()
}

/// Puts the manifest in front of the MLS ciphertext. A payload without files is unchanged.
pub(crate) fn wrap_manifest(entries: &[ManifestEntry], ciphertext: Vec<u8>) -> Result<Vec<u8>> {
    if entries.is_empty() {
        return Ok(ciphertext);
    }
    let body = postcard::to_allocvec(entries).map_err(|e| CoreError::Encoding(e.to_string()))?;
    let mut out = MANIFEST_MAGIC.to_vec();
    out.extend((body.len() as u32).to_be_bytes());
    out.extend(body);
    out.extend(ciphertext);
    Ok(out)
}

/// Splits the manifest off a payload. A payload without a well-formed manifest returns no entries
/// and unchanged bytes.
pub(crate) fn split_manifest(payload: &[u8]) -> (Vec<ManifestEntry>, &[u8]) {
    let Some(rest) = payload.strip_prefix(MANIFEST_MAGIC) else {
        return (Vec::new(), payload);
    };
    if rest.len() < 4 {
        return (Vec::new(), payload);
    }
    let len = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
    if rest.len() < 4 + len {
        return (Vec::new(), payload);
    }
    match postcard::from_bytes::<Vec<ManifestEntry>>(&rest[4..4 + len]) {
        Ok(entries) => (entries, &rest[4 + len..]),
        Err(_) => (Vec::new(), payload),
    }
}

/// True when the manifest describes exactly the files in the sealed text, in order.
pub(crate) fn manifest_matches(entries: &[ManifestEntry], files: &[Attached]) -> bool {
    entries.len() == files.len()
        && entries.iter().zip(files).all(|(e, a)| {
            e.hash == a.file.hash && e.cipher_size == a.file.cipher_size && e.seed == a.seed
        })
}

impl Engine {
    /// Mirrors the ciphertext of files named in a manifest, from the device that signed the event.
    /// Runs for any device with a mirror policy, including storage-only members that cannot read
    /// the sealed text. Files already mirrored, or being mirrored, are skipped.
    pub(crate) fn mirror_manifest(&self, device: dissent_identity::DeviceId, entries: &[ManifestEntry]) {
        let (policy, providers) = {
            let w = self.read();
            let Some((max_file_size, max_total)) = w.attach.mirror else {
                return;
            };
            let providers: Vec<_> = w
                .addresses
                .peers()
                .into_iter()
                .filter(|p| p.id == device)
                .filter_map(|p| p.to_iroh().ok())
                .collect();
            (
                MirrorPolicy {
                    max_file_size,
                    max_total,
                },
                providers,
            )
        };
        if providers.is_empty() {
            return;
        }
        for e in entries.iter().filter(|e| e.seed) {
            let id = data_encoding::BASE32_NOPAD.encode(&e.hash).to_lowercase();
            {
                let mut guard = self.write();
                if guard.attach.mirrored.contains_key(&id) || !guard.attach.mirroring.insert(id.clone()) {
                    continue;
                }
            }
            let engine = Engine {
                inner: self.inner.clone(),
            };
            let (hash, size) = (e.hash, e.cipher_size);
            let policy = MirrorPolicy {
                max_file_size: policy.max_file_size,
                max_total: policy.max_total,
            };
            let providers = providers.clone();
            tokio::spawn(async move {
                let node = engine.node();
                let result = engine
                    .inner
                    .files
                    .mirror(node.endpoint(), &providers, hash, size, policy, |_, _| {})
                    .await;
                let mut guard = engine.write();
                guard.attach.mirroring.remove(&id);
                match result {
                    Ok(()) => {
                        guard.attach.mirrored.insert(id, size);
                        if let Err(e) = guard.attach.save(&guard.paths) {
                            warn!(error = %e, "could not save mirror state");
                        }
                    }
                    Err(e) => debug!(error = %e, "not mirroring a file"),
                }
            });
        }
    }
}

impl Engine {
    /// Fetches the thumbnails of a message's images, for those small enough to fetch on arrival.
    fn auto_fetch_thumbnails(
        &self,
        community: Option<dissent_identity::CommunityId>,
        message: String,
        plain: &str,
    ) {
        let (_, files) = unpack(plain);
        for t in thumbs_of(plain).into_iter().filter(|t| t.file.size <= THUMB_AUTO_BYTES) {
            let Some(a) = files.iter().find(|a| a.file.hash == t.of) else {
                continue;
            };
            let id = attachment_id(&a.file);
            let fetch = {
                let mut guard = self.write();
                let w = &mut *guard;
                !thumb_path(&w.paths, &id).is_file() && w.thumb_fetching.insert(id.clone())
            };
            if !fetch {
                continue;
            }
            let engine = Engine {
                inner: self.inner.clone(),
            };
            let message = message.clone();
            tokio::spawn(async move {
                engine.fetch_thumbnail(community, message, id).await;
            });
        }
    }

    /// Tells the seeders about the thumbnails and preview pictures of a message this device has fetched.
    fn hint_media_of(&self, community: Option<dissent_identity::CommunityId>, message: &str) {
        let Some(c) = community else {
            return;
        };
        if let Ok((text, _)) = self.message_text(Some(c), message) {
            self.hint_seeders(c, media_blobs(&text));
        }
    }

    /// Fetches one thumbnail and tells the UI when it is on this device.
    async fn fetch_thumbnail(
        &self,
        community: Option<dissent_identity::CommunityId>,
        message: String,
        attachment: String,
    ) {
        let result = self.download_thumbnail(community, &message, &attachment).await;
        if result.is_ok() {
            self.hint_media_of(community, &message);
        }
        self.write().thumb_fetching.remove(&attachment);
        match result {
            Ok(()) => self.emit(EngineEvent::AttachmentChanged {
                message_id: message,
                attachment_id: attachment,
            }),
            Err(e) => debug!(error = %e, "could not fetch a thumbnail"),
        }
    }

    /// Fetches the pictures of a message's link previews, for those small enough to fetch on arrival.
    fn auto_fetch_embed_images(
        &self,
        community: Option<dissent_identity::CommunityId>,
        message: String,
        plain: &str,
    ) {
        for picture in embed_extras_of(plain).into_iter().filter_map(|e| e.image) {
            let id = attachment_id(&picture.file);
            let fetch = {
                let mut guard = self.write();
                let w = &mut *guard;
                !thumb_path(&w.paths, &id).is_file() && w.thumb_fetching.insert(id.clone())
            };
            if !fetch {
                continue;
            }
            let engine = Engine {
                inner: self.inner.clone(),
            };
            let message = message.clone();
            tokio::spawn(async move {
                engine.fetch_embed_image(community, message, id).await;
            });
        }
    }

    /// Fetches one link preview's picture and tells the UI when it is on this device.
    async fn fetch_embed_image(
        &self,
        community: Option<dissent_identity::CommunityId>,
        message: String,
        picture: String,
    ) {
        let result = self.download_embed_image(community, &message, &picture).await;
        if result.is_ok() {
            self.hint_media_of(community, &message);
        }
        self.write().thumb_fetching.remove(&picture);
        match result {
            Ok(()) => self.emit(EngineEvent::AttachmentChanged {
                message_id: message,
                attachment_id: picture,
            }),
            Err(e) => debug!(error = %e, "could not fetch a link preview picture"),
        }
    }

    /// Downloads and decrypts a link preview's picture into the thumbnails folder.
    async fn download_embed_image(
        &self,
        community: Option<dissent_identity::CommunityId>,
        message: &str,
        picture: &str,
    ) -> Result<()> {
        let (file, roots) = self.locate_embed_image(community, message, picture)?;
        self.download_picture(file, roots, picture, is_preview_image).await
    }

    /// Downloads and decrypts a thumbnail into the thumbnails folder, under the attachment's id.
    async fn download_thumbnail(
        &self,
        community: Option<dissent_identity::CommunityId>,
        message: &str,
        attachment: &str,
    ) -> Result<()> {
        let (file, roots) = self.locate_thumb(community, message, attachment)?;
        self.download_picture(file, roots, attachment, is_thumbnail).await
    }

    /// Downloads a picture blob and keeps it under `id` only if `valid` accepts what it decrypts to.
    async fn download_picture(
        &self,
        file: FileRef,
        roots: Vec<RootId>,
        id: &str,
        valid: fn(&[u8]) -> bool,
    ) -> Result<()> {
        let from: Vec<_> = self
            .providers_for(&roots)
            .into_iter()
            .filter_map(|p| p.to_iroh().ok())
            .collect();
        let node = self.node();
        let (target, scratch) = {
            let w = self.read();
            (
                thumb_path(&w.paths, id),
                w.paths.thumbnails_dir().join(format!("{id}.part")),
            )
        };
        // Each picture decrypts into its own folder, so two fetches cannot pick the same name.
        std::fs::create_dir_all(&scratch)?;
        let fetched = self
            .inner
            .files
            .fetch(node.endpoint(), &from, &file, &scratch, |_, _| {})
            .await;
        let result = fetched.map_err(files_err).and_then(|path| {
            // The sender chose these bytes. Keep them only if they are a picture this device could make.
            let data = std::fs::read(&path)?;
            if !valid(&data) {
                return Err(CoreError::Invalid("the blob is not a picture this device accepts".into()));
            }
            std::fs::rename(&path, &target)?;
            Ok(())
        });
        let _ = std::fs::remove_dir_all(&scratch);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(hash: u8, cipher_size: u64) -> Attached {
        Attached {
            file: FileRef {
                hash: [hash; 32],
                size: cipher_size.saturating_sub(16),
                cipher_size,
                name: "a.txt".into(),
                mime: "text/plain".into(),
                key: [9; 32],
                nonce_prefix: [1; 8],
            },
            from: RootId([7; 32]),
            seed: true,
        }
    }

    #[tokio::test]
    async fn the_default_download_folder_is_in_place_when_the_engine_starts() {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("dissent-core-default-dir-{}-{nanos}", std::process::id()));
        let os = dir.join("OS-Downloads");
        let identity = dissent_identity::Identity::create(now_ms());
        let net = dissent_net::NodeConfig {
            relay: dissent_net::RelayChoice::Disabled,
            loopback_only: true,
            ..dissent_net::NodeConfig::default()
        };
        let (engine, _events) = Engine::start_with_options(
            identity,
            dir.clone(),
            net,
            crate::test_support::vault_for(&dir),
            crate::Timings::default(),
            Some(os.clone()),
        )
        .await
        .expect("engine starts");
        let settings = engine.transfer_settings();
        assert_eq!(settings.download_dir, os.to_string_lossy());
        assert!(settings.download_dir_is_default);
        engine.shutdown().await.expect("engine stops");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_picked_folder_wins_over_the_os_folder_and_the_os_folder_over_the_profile() {
        let profile = Path::new("/profile/downloads");
        let os = Path::new("/home/ada/Downloads");
        let picked = Path::new("/mnt/shared");
        assert_eq!(resolve_download_dir(Some(picked), Some(os), profile), picked, "a picked folder wins");
        assert_eq!(resolve_download_dir(None, Some(os), profile), os, "reset to the default gives the OS folder");
        assert_eq!(resolve_download_dir(None, None, profile), profile, "no OS folder gives the profile folder");
    }

    #[test]
    fn manifest_round_trips_in_front_of_the_ciphertext() {
        let files = vec![file(1, 100), file(2, 200)];
        let manifest = manifest_of(&pack("text", &files).unwrap());
        let wrapped = wrap_manifest(&manifest, b"mls bytes".to_vec()).unwrap();
        let (got, rest) = split_manifest(&wrapped);
        assert_eq!(got, manifest);
        assert_eq!(rest, b"mls bytes");
    }

    #[test]
    fn a_payload_without_files_is_unchanged() {
        let wrapped = wrap_manifest(&[], b"mls bytes".to_vec()).unwrap();
        assert_eq!(wrapped, b"mls bytes");
        let (got, rest) = split_manifest(&wrapped);
        assert!(got.is_empty());
        assert_eq!(rest, b"mls bytes");
    }

    #[test]
    fn a_truncated_manifest_is_treated_as_absent() {
        let manifest = manifest_of(&pack("t", &[file(1, 100)]).unwrap());
        let wrapped = wrap_manifest(&manifest, b"x".to_vec()).unwrap();
        let (got, rest) = split_manifest(&wrapped[..wrapped.len() - 3]);
        assert!(got.is_empty());
        assert_eq!(rest.len(), wrapped.len() - 3);
    }

    #[test]
    fn the_manifest_matches_the_sealed_files() {
        let files = vec![file(1, 100), file(2, 200)];
        let manifest = manifest_of(&pack("t", &files).unwrap());
        assert!(manifest_matches(&manifest, &files));
    }

    #[test]
    fn a_sender_cannot_advertise_a_different_blob() {
        // The manifest names blob A, but the sealed text carries blob B.
        let sealed = vec![file(0xB, 100)];
        let advertised = manifest_of(&pack("t", &[file(0xA, 100)]).unwrap());
        assert!(!manifest_matches(&advertised, &sealed), "a different hash is refused");
    }

    #[test]
    fn a_sender_cannot_understate_the_size_or_the_seed_flag() {
        let sealed = vec![file(1, 100)];
        let mut smaller = manifest_of(&pack("t", &sealed).unwrap());
        smaller[0].cipher_size = 50;
        assert!(!manifest_matches(&smaller, &sealed), "a different size is refused");
        let mut flipped = manifest_of(&pack("t", &sealed).unwrap());
        flipped[0].seed = false;
        assert!(!manifest_matches(&flipped, &sealed), "a different seed flag is refused");
    }

    #[test]
    fn a_sender_cannot_add_or_drop_entries() {
        let sealed = vec![file(1, 100), file(2, 200)];
        let one = manifest_of(&pack("t", &[file(1, 100)]).unwrap());
        assert!(!manifest_matches(&one, &sealed), "a missing entry is refused");
        let mut extra = manifest_of(&pack("t", &sealed).unwrap());
        extra.push(extra[0].clone());
        assert!(!manifest_matches(&extra, &sealed), "an extra entry is refused");
        let reordered = vec![
            manifest_of(&pack("t", &[file(2, 200)]).unwrap())[0].clone(),
            manifest_of(&pack("t", &[file(1, 100)]).unwrap())[0].clone(),
        ];
        assert!(!manifest_matches(&reordered, &sealed), "a reordered manifest is refused");
    }
}

/// Transfer settings, as the UI shows them.
#[derive(Clone, Debug, serde::Serialize)]
pub struct TransferSettings {
    /// Files at or under this many bytes download on arrival. `None` means always ask.
    pub auto_accept_bytes: Option<u64>,
    /// Where finished downloads are written.
    pub download_dir: String,
    /// True until the user picks a folder. Then the reset to the default is offered.
    pub download_dir_is_default: bool,
}

/// What the encrypted file store holds on this device.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct StoreStats {
    /// Ciphertext bytes this device keeps pinned.
    pub used_bytes: u64,
    /// Files this device keeps pinned.
    pub files: u64,
    /// The most ciphertext the mirror policy allows in total. `0` when this device does not mirror.
    pub budget_bytes: u64,
    /// Ciphertext of the files this device shared. Taken from its upload records.
    pub shared_bytes: u64,
    /// Ciphertext of the downloads that finished.
    pub downloaded_bytes: u64,
    /// Ciphertext held so far for downloads still in progress.
    pub partial_bytes: u64,
    /// Bytes of the thumbnails this device keeps for pictures in messages. They are plaintext JPEGs.
    pub thumbnail_bytes: u64,
}

/// The size of the files in a thumbnails folder. A missing folder counts as empty.
fn thumbnail_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| e.metadata().ok())
                .filter(|m| m.is_file())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

/// Attachment ids that a message still shows: those in messages that are not deleted, in
/// conversations that are not left.
fn live_attachment_ids(w: &World) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    let mut add = |text: &str| {
        for a in unpack(text).1 {
            ids.insert(attachment_id(&a.file));
        }
        for picture in embed_extras_of(text).into_iter().filter_map(|e| e.image) {
            ids.insert(attachment_id(&picture.file));
        }
    };
    for c in w.communities.values() {
        let deleted: BTreeSet<dissent_events::EventId> = c
            .state()
            .messages
            .values()
            .flatten()
            .filter(|m| m.deleted)
            .map(|m| m.id)
            .collect();
        for (id, (_, text)) in &c.content {
            if !deleted.contains(id) {
                add(text);
            }
        }
    }
    for (conv, msgs) in &w.conv_msgs {
        if w.conv_book.convs.get(conv).is_some_and(|meta| meta.left) {
            continue;
        }
        let acts = w.conv_acts.get(conv).map_or(&[][..], Vec::as_slice);
        let deleted = crate::conv_actions::deleted_messages(acts, msgs);
        for m in msgs.iter().filter(|m| !deleted.contains(&m.id)) {
            add(&m.text);
        }
    }
    ids
}

impl Engine {
    /// Deletes the thumbnails of attachments that no live message shows any more, such as the
    /// thumbnails of deleted messages and of left conversations. Runs at start and after a local
    /// delete or leave. Other devices' deletions are picked up at the next start.
    pub(crate) fn prune_thumbnails(&self) {
        let (dir, live) = {
            let w = self.read();
            (w.paths.thumbnails_dir(), live_attachment_ids(&w))
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let id = name.strip_suffix(".jpg").unwrap_or(&name);
            if live.contains(id) {
                continue;
            }
            let path = entry.path();
            let removed = if path.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
            if let Err(e) = removed {
                debug!(error = %e, "could not remove a stale thumbnail");
            }
        }
    }
}
impl Engine {
    pub fn transfer_settings(&self) -> TransferSettings {
        let w = self.read();
        TransferSettings {
            auto_accept_bytes: w.attach.auto_accept_bytes,
            download_dir: resolve_download_dir(w.attach.download_dir.as_deref(), w.attach.default_download_dir.as_deref(), &w.paths.downloads())
                .to_string_lossy()
                .into_owned(),
            download_dir_is_default: w.attach.download_dir.is_none(),
        }
    }

    /// Sets the auto-download limit. `None` asks every time.
    pub fn set_transfer_settings(&self, auto_accept_bytes: Option<u64>) -> Result<TransferSettings> {
        {
            let mut guard = self.write();
            guard.attach.auto_accept_bytes = auto_accept_bytes;
            guard.attach.save(&guard.paths)?;
        }
        Ok(self.transfer_settings())
    }

    /// Usage of the encrypted file store.
    pub async fn store_stats(&self) -> Result<StoreStats> {
        let (used_bytes, files) = self.inner.files.pinned_stats().await.map_err(files_err)?;
        let budget_bytes = self.read().attach.mirror.map_or(0, |(_, total)| total);
        let (shared_bytes, downloaded_bytes, partial_bytes) = {
            let w = self.read();
            let mut shared = 0u64;
            let mut done = 0u64;
            let mut partial = 0u64;
            for t in w.attach.transfers.values() {
                let size = t.file.cipher_size;
                match (t.direction.as_str(), t.state.as_str()) {
                    ("upload", _) => shared += size,
                    ("download", "done") => done += size,
                    ("download", _) => {
                        partial += (t.progress.clamp(0.0, 1.0) * size as f64) as u64;
                    }
                    _ => {}
                }
            }
            (shared, done, partial)
        };
        Ok(StoreStats {
            used_bytes,
            files,
            budget_bytes,
            shared_bytes,
            downloaded_bytes,
            partial_bytes,
            thumbnail_bytes: thumbnail_bytes(&self.read().paths.thumbnails_dir()),
        })
    }

    /// Starts downloads for the files in a message that arrived, when they are within the
    /// auto-download limit. `message` is the id `download_attachment` takes.
    pub(crate) fn auto_download(&self, community: Option<dissent_identity::CommunityId>, message: String, plain: &str) {
        self.auto_fetch_thumbnails(community, message.clone(), plain);
        self.auto_fetch_embed_images(community, message.clone(), plain);
        let limit = match self.read().attach.auto_accept_bytes {
            Some(limit) => limit,
            None => return,
        };
        let (_, files) = unpack(plain);
        for a in files.into_iter().filter(|a| a.file.size <= limit) {
            let id = attachment_id(&a.file);
            {
                let w = self.read();
                if w.attach.downloads.contains_key(&id) || w.attach.transfers.contains_key(&id) {
                    continue;
                }
            }
            let engine = Engine {
                inner: self.inner.clone(),
            };
            let message = message.clone();
            tokio::spawn(async move {
                if let Err(e) = engine.download_attachment(community, message, id).await {
                    debug!(error = %e, "could not start an automatic download");
                }
            });
        }
    }
}

impl Engine {
    /// The folder finished downloads are written to.
    pub(crate) fn download_dir(&self) -> PathBuf {
        let w = self.read();
        resolve_download_dir(w.attach.download_dir.as_deref(), w.attach.default_download_dir.as_deref(), &w.paths.downloads())
    }

    /// Every file this device saved from a download, in any folder it has used. The app opens and reveals only these.
    pub fn saved_files(&self) -> Vec<PathBuf> {
        self.read().attach.downloads.values().cloned().collect()
    }

    /// Sets the folder finished downloads are written to, or resets it to the default with `None`.
    /// The folder must already exist and accept new files.
    pub fn set_download_dir(&self, dir: Option<PathBuf>) -> Result<TransferSettings> {
        if let Some(dir) = &dir {
            check_writable_dir(dir)?;
        }
        {
            let mut guard = self.write();
            guard.attach.download_dir = dir;
            guard.attach.save(&guard.paths)?;
        }
        Ok(self.transfer_settings())
    }
}

/// The folder finished downloads go to: a folder the user picked, else the OS Downloads folder, else the profile's
/// own downloads folder.
pub(crate) fn resolve_download_dir(picked: Option<&Path>, os_default: Option<&Path>, profile: &Path) -> PathBuf {
    picked.or(os_default).unwrap_or(profile).to_path_buf()
}

/// Fails unless `dir` is an existing directory that this process can write a file into.
fn check_writable_dir(dir: &Path) -> Result<()> {
    if !dir.is_dir() {
        return Err(CoreError::Invalid(format!("{} is not an existing folder", dir.display())));
    }
    let probe = dir.join(".dissent-write-check");
    std::fs::write(&probe, b"ok").map_err(|e| {
        CoreError::Invalid(format!("{} is not writable: {e}", dir.display()))
    })?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// Tests of the payload format. The v1 bytes are what the release before thumbnails wrote, and
/// they must keep opening and keep being written unchanged.
#[cfg(test)]
mod wire_tests {
    use super::*;

    /// `pack("hello there", [a.txt])` from the release before thumbnails, as the sealed text.
    const V1: &str = "\u{1e}dissent-attachments/v1\u{1e}C2hlbGxvIHRoZXJlAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBVGQFYS50eHQKdGV4dC9wbGFpbgkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJAQEBAQEBAQEHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwEA";

    fn a_file() -> Attached {
        Attached {
            file: FileRef {
                hash: [1; 32],
                size: 84,
                cipher_size: 100,
                name: "a.txt".into(),
                mime: "text/plain".into(),
                key: [9; 32],
                nonce_prefix: [1; 8],
            },
            from: RootId([7; 32]),
            seed: true,
        }
    }

    fn a_thumb() -> Thumb {
        Thumb {
            of: [1; 32],
            file: FileRef {
                hash: [2; 32],
                size: 40,
                cipher_size: 56,
                name: "thumbnail.jpg".into(),
                mime: "image/jpeg".into(),
                key: [3; 32],
                nonce_prefix: [4; 8],
            },
            width: 320,
            height: 160,
        }
    }

    #[test]
    fn a_payload_from_the_release_before_thumbnails_still_opens() {
        let (text, files) = unpack(V1);
        assert_eq!(text, "hello there");
        assert_eq!(files, vec![a_file()]);
        assert!(thumbs_of(V1).is_empty());
    }

    #[test]
    fn a_message_without_thumbnails_is_written_as_before() {
        assert_eq!(pack("hello there", &[a_file()]).unwrap(), V1);
        assert_eq!(pack_full("hello there", &[a_file()], &[], &[]).unwrap(), V1);
    }

    #[test]
    fn thumbnails_round_trip_with_their_files() {
        let sealed = pack_full("look", &[a_file()], &[], &[a_thumb()]).unwrap();
        assert_eq!(unpack(&sealed), ("look".to_string(), vec![a_file()]));
        assert_eq!(thumbs_of(&sealed), vec![a_thumb()]);
    }

    #[test]
    fn a_reader_of_the_old_format_ignores_the_thumbnail_trailer() {
        // The old reader decodes `Packed` and stops. The bytes after it are not an error.
        let sealed = pack_full("look", &[a_file()], &[], &[a_thumb()]).unwrap();
        let bytes = data_encoding::BASE64URL_NOPAD
            .decode(sealed.strip_prefix(MARK).unwrap().as_bytes())
            .unwrap();
        let old: Packed = postcard::from_bytes(&bytes).expect("the old reader opens the payload");
        assert_eq!(old.text, "look");
        assert_eq!(old.files, vec![a_file()]);
    }

    #[test]
    fn a_sender_cannot_make_us_fetch_a_large_or_odd_thumbnail() {
        // Each of these is a thumbnail a sender could write. None is trusted, and the message still opens.
        let mut big = a_thumb();
        big.file.size = 70_000;
        big.file.cipher_size = dissent_files::cipher_len(70_000);
        let mut wide = a_thumb();
        wide.width = 4_000;
        let mut no_size = a_thumb();
        no_size.height = 0;
        let mut mismatched = a_thumb();
        mismatched.file.cipher_size += 1;
        for bad in [big, wide, no_size, mismatched] {
            let sealed = pack_full("look", &[a_file()], &[], &[bad]).unwrap();
            assert!(thumbs_of(&sealed).is_empty());
            assert_eq!(unpack(&sealed), ("look".to_string(), vec![a_file()]));
        }
    }

    #[test]
    fn a_damaged_trailer_leaves_the_text_and_files_readable() {
        let sealed = pack_full("look", &[a_file()], &[], &[a_thumb()]).unwrap();
        let mut bytes = data_encoding::BASE64URL_NOPAD
            .decode(sealed.strip_prefix(MARK).unwrap().as_bytes())
            .unwrap();
        let (_, rest) = postcard::take_from_bytes::<Packed>(&bytes).unwrap();
        let start = bytes.len() - rest.len();
        bytes.truncate(start + 3);
        let damaged = format!("{MARK}{}", data_encoding::BASE64URL_NOPAD.encode(&bytes));
        assert_eq!(unpack(&damaged), ("look".to_string(), vec![a_file()]));
        assert!(thumbs_of(&damaged).is_empty());
    }

    /// A payload whose trailer holds the given extensions after `Packed`, as a newer sender writes it.
    fn sealed_with(exts: &[(u8, Vec<u8>)]) -> String {
        let mut bytes = postcard::to_allocvec(&Packed {
            text: "look".into(),
            files: vec![a_file()],
            embeds: Vec::new(),
        })
        .unwrap();
        bytes.extend_from_slice(&postcard::to_allocvec(&Trailer { exts: exts.to_vec() }).unwrap());
        format!("{MARK}{}", data_encoding::BASE64URL_NOPAD.encode(&bytes))
    }

    #[test]
    fn a_trailer_tag_this_reader_does_not_know_is_skipped() {
        // A tag from a newer sender (here `mentions`) is skipped, and the thumbnails after it still arrive.
        let thumbs = postcard::to_allocvec(&vec![a_thumb()]).unwrap();
        let sealed = sealed_with(&[(200, vec![0xff; 5]), (EXT_THUMBS, thumbs)]);
        assert_eq!(unpack(&sealed), ("look".to_string(), vec![a_file()]));
        assert_eq!(thumbs_of(&sealed), vec![a_thumb()]);
    }

    #[test]
    fn mentions_travel_in_the_trailer_and_the_text_stays_plain() {
        let bob = [7u8; 32];
        let sealed = pack_all("hi @bob", &[], &[], &[], Some(&Mentions { users: vec![bob], everyone: false })).unwrap();
        assert_eq!(text_of(&sealed), "hi @bob");
        assert_eq!(mentions_of(&sealed), Mentions { users: vec![bob], everyone: false });
    }

    #[test]
    fn a_message_without_mentions_packs_the_same_bytes_as_before() {
        let plain = pack_all("hello", &[], &[], &[], Some(&Mentions::default())).unwrap();
        assert_eq!(plain, pack_full("hello", &[], &[], &[]).unwrap());
        assert!(mentions_of(&plain).is_empty());
    }

    #[test]
    fn a_mentions_body_that_does_not_decode_is_ignored_and_thumbnails_still_arrive() {
        let thumbs = postcard::to_allocvec(&vec![a_thumb()]).unwrap();
        let sealed = sealed_with(&[(EXT_MENTIONS, vec![0xff, 0xff]), (EXT_THUMBS, thumbs)]);
        assert!(mentions_of(&sealed).is_empty());
        assert_eq!(thumbs_of(&sealed), vec![a_thumb()]);
    }

    #[test]
    fn a_message_before_mentions_existed_has_none() {
        assert!(mentions_of(&sealed_with(&[(EXT_THUMBS, postcard::to_allocvec(&vec![a_thumb()]).unwrap())])).is_empty());
        assert!(mentions_of("plain text").is_empty());
    }

    #[test]
    fn a_trailer_with_only_unknown_tags_gives_no_thumbnails() {
        let sealed = sealed_with(&[(200, vec![1, 2, 3])]);
        assert_eq!(unpack(&sealed), ("look".to_string(), vec![a_file()]));
        assert!(thumbs_of(&sealed).is_empty());
    }

    #[test]
    fn a_thumbs_body_that_does_not_decode_is_skipped_alone() {
        let sealed = sealed_with(&[
            (EXT_THUMBS, vec![0xff, 0xff]),
            (EXT_THUMBS, postcard::to_allocvec(&vec![a_thumb()]).unwrap()),
        ]);
        assert_eq!(thumbs_of(&sealed), vec![a_thumb()]);
    }
}

#[cfg(test)]
mod source_view_tests {
    use super::*;
    use dissent_files::PIECE_BYTES;
    use dissent_identity::DeviceKey;

    fn provider() -> PeerAddr {
        PeerAddr {
            id: DeviceKey::generate().id(),
            relay: None,
            direct: Vec::new(),
        }
    }

    fn live(providers: Vec<PeerAddr>, counts: Vec<(u64, u64)>, cipher: u64) -> LiveSources {
        LiveSources {
            providers,
            counts: counts
                .into_iter()
                .enumerate()
                .map(|(index, (bytes, pieces))| SourceCount {
                    index,
                    bytes,
                    pieces,
                })
                .collect(),
            cipher,
        }
    }

    #[test]
    fn source_views_keep_the_order_providers_were_tried_in() {
        let (a, b) = (provider(), provider());
        let live = live(vec![a.clone(), b.clone()], vec![(100, 1), (300, 4)], 1024);
        let views = source_views(&live, "waiting", |p| format!("name-{}", p.id.suffix()));
        assert_eq!(views.len(), 2);
        assert_eq!(views[0].device, a.id.to_string());
        assert_eq!((views[0].bytes, views[0].verified_pieces), (100, 1));
        assert_eq!(views[1].device, b.id.to_string());
        assert_eq!((views[1].bytes, views[1].verified_pieces), (300, 4));
        assert!(views.iter().all(|v| v.state == "waiting"));
    }

    #[test]
    fn pieces_total_comes_from_the_ciphertext_size() {
        let cipher = 2 * PIECE_BYTES + 1;
        let live = live(
            vec![provider(), provider()],
            vec![(PIECE_BYTES, 1), (PIECE_BYTES + 1, 2)],
            cipher,
        );
        assert_eq!(
            piece_counts(&live),
            PieceCounts {
                total: 3,
                verified: 3
            }
        );
    }

    #[test]
    fn only_providers_that_delivered_bytes_are_listed_with_a_rate() {
        let sources = vec![
            SourceView {
                device: "a".into(),
                name: "Ada".into(),
                state: "waiting".into(),
                bytes: 2_000,
                verified_pieces: 1,
            },
            SourceView {
                device: "b".into(),
                name: "Bo".into(),
                state: "waiting".into(),
                bytes: 0,
                verified_pieces: 0,
            },
        ];
        let listed = file_sources(&sources, 4_000);
        assert_eq!(
            listed,
            vec![FileSourceView {
                display_name: "Ada".into(),
                route: "direct".into(),
                rate_bps: 500
            }]
        );
    }

    #[test]
    fn a_zero_elapsed_time_gives_a_zero_rate() {
        let sources = vec![SourceView {
            device: "a".into(),
            name: "Ada".into(),
            state: "done".into(),
            bytes: 9,
            verified_pieces: 0,
        }];
        assert_eq!(file_sources(&sources, 0)[0].rate_bps, 0);
    }
}

#[cfg(test)]
mod preview_extra_tests {
    use super::*;

    fn a_preview() -> crate::previews::Embed {
        crate::previews::Embed {
            url: "https://a.example/x".into(),
            site: "a.example".into(),
            title: "Title".into(),
            description: None,
        }
    }

    fn a_picture(cipher_size: u64) -> EmbedImage {
        EmbedImage {
            file: FileRef {
                hash: [4; 32],
                size: cipher_size.saturating_sub(16),
                cipher_size,
                name: "preview.jpg".into(),
                mime: "image/jpeg".into(),
                key: [9; 32],
                nonce_prefix: [1; 8],
            },
            width: 640,
            height: 320,
        }
    }

    #[test]
    fn a_preview_picture_and_theme_round_trip_by_position() {
        let extras = vec![EmbedExtra { index: 0, image: Some(a_picture(1000)), theme: Some([1, 2, 3]) }];
        let sealed = pack_all_with("look", &[], &[a_preview()], &[], None, &extras).unwrap();
        assert_eq!(embed_extras_of(&sealed), extras);
        assert_eq!(embeds_of(&sealed), vec![a_preview()]);
    }

    #[test]
    fn an_entry_for_no_preview_or_an_oversized_picture_is_dropped() {
        let no_preview = vec![EmbedExtra { index: 4, image: Some(a_picture(1000)), theme: None }];
        let sealed = pack_all_with("look", &[], &[a_preview()], &[], None, &no_preview).unwrap();
        assert!(embed_extras_of(&sealed).is_empty());

        let big = EmbedExtra { index: 0, image: Some(a_picture(PREVIEW_IMAGE_MAX_BYTES as u64 + 1)), theme: None };
        let sealed = pack_all_with("look", &[], &[a_preview()], &[], None, &[big]).unwrap();
        assert!(embed_extras_of(&sealed).is_empty(), "a picture over the auto-fetch cap is not kept");
    }

    #[test]
    fn an_unknown_extension_beside_the_preview_pictures_is_skipped() {
        let body = postcard::to_allocvec(&vec![EmbedExtra { index: 0, image: Some(a_picture(1000)), theme: None }]).unwrap();
        let mut bytes = postcard::to_allocvec(&Packed {
            text: "look".into(),
            files: Vec::new(),
            embeds: vec![a_preview()],
        })
        .unwrap();
        bytes.extend_from_slice(
            &postcard::to_allocvec(&Trailer { exts: vec![(200, vec![0xff; 4]), (EXT_EMBEDS, body)] }).unwrap(),
        );
        let sealed = format!("{MARK}{}", data_encoding::BASE64URL_NOPAD.encode(&bytes));
        assert_eq!(embed_extras_of(&sealed).len(), 1);
        assert_eq!(text_of(&sealed), "look");
    }
}
