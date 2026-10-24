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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use dissent_files::{FileRef, FilesError, Input, MirrorPolicy, MAX_FILE_SIZE};
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

/// Wraps message text and its files into the sealed payload.
pub(crate) fn pack(text: &str, files: &[Attached]) -> Result<String> {
    pack_with_embeds(text, files, &[])
}

/// Packs text, files and link previews into a sealed payload.
pub(crate) fn pack_with_embeds(
    text: &str,
    files: &[Attached],
    embeds: &[crate::previews::Embed],
) -> Result<String> {
    let bytes = postcard::to_allocvec(&Packed {
        text: text.to_string(),
        files: files.to_vec(),
        embeds: embeds.to_vec(),
    })
    .map_err(|e| CoreError::Encoding(e.to_string()))?;
    Ok(format!("{MARK}{}", data_encoding::BASE64URL_NOPAD.encode(&bytes)))
}

/// Splits a sealed payload into text and files. Plain text has no files.
pub(crate) fn unpack(plain: &str) -> (String, Vec<Attached>) {
    let Some(body) = plain.strip_prefix(MARK) else {
        return (plain.to_string(), Vec::new());
    };
    let decoded = data_encoding::BASE64URL_NOPAD
        .decode(body.as_bytes())
        .ok()
        .and_then(|bytes| postcard::from_bytes::<Packed>(&bytes).ok());
    match decoded {
        Some(p) => (p.text, p.files),
        None => (String::new(), Vec::new()),
    }
}

/// The link previews in a sealed payload.
pub(crate) fn embeds_of(plain: &str) -> Vec<crate::previews::Embed> {
    let Some(body) = plain.strip_prefix(MARK) else {
        return Vec::new();
    };
    data_encoding::BASE64URL_NOPAD
        .decode(body.as_bytes())
        .ok()
        .and_then(|bytes| postcard::from_bytes::<Packed>(&bytes).ok())
        .map(|p| p.embeds)
        .unwrap_or_default()
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
}

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
    /// The providers the transfer may use. Per-source bytes are not reported by the file store.
    pub sources: Vec<SourceView>,
}

/// One provider of a transfer.
#[derive(Clone, Debug, serde::Serialize)]
pub struct SourceView {
    pub device: String,
    pub name: String,
    /// `waiting` while the transfer runs, and `done` once it finishes. The file store does not say
    /// which provider served the bytes.
    pub state: String,
    /// Unknown per provider, so always 0.
    pub bytes: u64,
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
    /// Where finished downloads go. `None` means the default downloads folder.
    pub download_dir: Option<PathBuf>,
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
        files
            .iter()
            .map(|a| {
                let id = attachment_id(&a.file);
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
                AttachmentView {
                    id,
                    name: a.file.safe_name(),
                    size: a.file.size,
                    mime: a.file.mime.clone(),
                    needed: state != "downloaded",
                    state,
                    progress,
                    local_path,
                    seed: a.seed,
                }
            })
            .collect()
    }
}

impl Engine {
    /// Encrypts the files and returns them ready to pack into a message.
    pub(crate) async fn add_files(&self, paths: &[PathBuf], seed: bool) -> Result<Vec<Attached>> {
        let mut out = Vec::new();
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
            out.push(Attached {
                file,
                from: self.inner.me,
                seed,
            });
        }
        Ok(out)
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
        let attached = self.add_files(&paths, seed).await?;
        let plain = pack(text.trim(), &attached)?;
        self.send_channel_plain(community, channel, plain).await
    }

    /// Sends files to a conversation. `text` may be empty.
    pub async fn send_conversation_files(
        &self,
        conv: crate::groups::ConvId,
        paths: Vec<PathBuf>,
        text: String,
    ) -> Result<crate::views::DmItem> {
        let attached = self.add_files(&paths, true).await?;
        let plain = pack(text.trim(), &attached)?;
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

    /// The file, providers and message id for an attachment, found in the message text.
    fn locate_attachment(
        &self,
        community: Option<dissent_identity::CommunityId>,
        message: &str,
        attachment: &str,
    ) -> Result<(FileRef, Vec<RootId>, String)> {
        let w = self.read();
        let (text, mut roots) = match community {
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
                (text, roots)
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
                (m.text.clone(), roots)
            }
        };
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
        self.cancel_transfer(id)
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
        let from: Vec<_> = providers
            .iter()
            .filter_map(|p| p.parse::<PeerAddr>().ok())
            .filter_map(|p| p.to_iroh().ok())
            .collect();
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
        let result = self
            .inner
            .files
            .fetch(node.endpoint(), &from, &file, &dest, move |held, total| {
                let progress = if total == 0 { 0.0 } else { held as f64 / total as f64 };
                engine.set_progress(&att, progress, &msg);
            })
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

fn view_of(t: &Transfer, w: &World) -> TransferView {
    let sources = t
        .providers
        .iter()
        .filter_map(|p| p.parse::<PeerAddr>().ok())
        .map(|p| {
            let name = w
                .directory
                .root_of(&p.id)
                .map(|r| w.display_name(&r))
                .unwrap_or_else(|| p.id.to_string());
            SourceView {
                device: p.id.to_string(),
                name,
                state: if t.state == "done" { "done" } else { "waiting" }.to_string(),
                bytes: 0,
            }
        })
        .collect();
    TransferView {
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
}

impl Engine {
    pub fn transfer_settings(&self) -> TransferSettings {
        let w = self.read();
        TransferSettings {
            auto_accept_bytes: w.attach.auto_accept_bytes,
            download_dir: w.attach.download_dir.clone().unwrap_or_else(|| w.paths.downloads()).to_string_lossy().into_owned(),
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
        })
    }

    /// Starts downloads for the files in a message that arrived, when they are within the
    /// auto-download limit. `message` is the id `download_attachment` takes.
    pub(crate) fn auto_download(&self, community: Option<dissent_identity::CommunityId>, message: String, plain: &str) {
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
        w.attach
            .download_dir
            .clone()
            .unwrap_or_else(|| w.paths.downloads())
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
