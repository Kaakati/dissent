//! Disk layout and file helpers.
//!
//! Logs are repeated `[u32 BE length][postcard record]`, appended in one write.
//! `.bin` files are replaced whole, through a temp file and a rename.

use crate::error::{CoreError, Result};
use dissent_identity::{CommunityId, RootId, Vault};
use serde::{Serialize, de::DeserializeOwned};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

/// Locations of everything under the data directory.
#[derive(Clone, Debug)]
pub(crate) struct Paths {
    root: PathBuf,
}

impl Paths {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
    pub fn profile(&self) -> PathBuf {
        self.root.join("profile.bin")
    }
    pub fn contacts(&self) -> PathBuf {
        self.root.join("contacts.bin")
    }
    /// This device's own profile picture, if it has one.
    pub fn own_avatar(&self) -> PathBuf {
        self.root.join("avatar.bin")
    }
    /// Pictures received directly from contacts, by root, with the HLC of each.
    pub fn peer_avatars(&self) -> PathBuf {
        self.root.join("peer_avatars.bin")
    }
    /// This device's profile banner and the banners contacts sent directly. See `profile_banners`.
    pub fn banners(&self) -> PathBuf {
        self.root.join("banners.bin")
    }
    /// Decrypted pictures, one file per content hash.
    pub fn avatars_dir(&self) -> PathBuf {
        self.root.join("avatars")
    }
    /// Decrypted thumbnails of image attachments, one JPEG per attachment id.
    pub fn thumbnails_dir(&self) -> PathBuf {
        self.root.join("thumbnails")
    }
    /// Inbox service state.
    pub fn inbox(&self) -> PathBuf {
        self.root.join("inbox.bin")
    }
    /// Conversations: kinds, members, queued packets and pending text.
    pub fn conv_book(&self) -> PathBuf {
        self.root.join("conversations.bin")
    }
    /// Stored text messages of one conversation.
    pub fn conv_log(&self, id: &crate::groups::ConvId) -> PathBuf {
        self.root.join("conversations").join(format!("{id}.log"))
    }
    /// Stored actions of one conversation: replies, reactions, edits and deletes. See `conv_actions.rs`.
    pub fn conv_acts(&self, id: &crate::groups::ConvId) -> PathBuf {
        self.root.join("conversations").join(format!("{id}.acts"))
    }
    /// Each channel's mention read times. See `mention_reads.rs`.
    pub fn mention_reads(&self) -> PathBuf {
        self.root.join("mention_reads.bin")
    }
    /// Packets of messages a recipient or an inbox refused, kept so the sender can retry them.
    pub fn refused_packets(&self) -> PathBuf {
        self.root.join("refused_packets.bin")
    }
    /// When each contact was added, and when we first saw each person. See `contact_times.rs`.
    pub fn contact_times(&self) -> PathBuf {
        self.root.join("contact_times.bin")
    }
    /// Contact requests, accepts we owe, declines and the DM policy.
    pub fn contact_book(&self) -> PathBuf {
        self.root.join("contact_book.bin")
    }
    /// "Let people add me by address" turned off. A separate file, so the contact book keeps its layout.
    pub fn address_privacy(&self) -> PathBuf {
        self.root.join("address_privacy.bin")
    }
    /// "Show my friends list to my friends" turned off. A separate file, as `address_privacy` is.
    pub fn friends_hidden(&self) -> PathBuf {
        self.root.join("friends_hidden.bin")
    }
    /// The last history-readiness answer of each community, so a restart starts from it. See `history`.
    pub fn history_ready(&self) -> PathBuf {
        self.root.join("history_ready.bin")
    }
    /// Identity changes seen for the people we talk to, so the notice survives a restart.
    pub fn identity_changes(&self) -> PathBuf {
        self.root.join("identity_changes.log")
    }
    /// Peer addresses learned from invites, contacts and requests, so a restart can still dial.
    pub fn addresses(&self) -> PathBuf {
        self.root.join("addresses.bin")
    }
    /// Our own latest signed address. Its timestamp keeps announcements increasing across restarts.
    pub fn own_addr(&self) -> PathBuf {
        self.root.join("own_addr.bin")
    }
    pub fn communities_dir(&self) -> PathBuf {
        self.root.join("communities")
    }
    pub fn community_dir(&self, id: &CommunityId) -> PathBuf {
        self.communities_dir().join(id.to_string())
    }
    pub fn genesis(&self, id: &CommunityId) -> PathBuf {
        self.community_dir(id).join("genesis.bin")
    }
    /// The community secret, shared in its invites.
    pub fn access(&self, id: &CommunityId) -> PathBuf {
        self.community_dir(id).join("access.bin")
    }
    /// The control keys of each MLS epoch this device was in. Sealed. See `epoch_keys`.
    pub fn epoch_keys(&self, id: &CommunityId) -> PathBuf {
        self.community_dir(id).join("epoch-keys.bin")
    }
    /// The history key of each epoch this device was in, sealed. See `history`.
    pub fn history_keys(&self, id: &CommunityId) -> PathBuf {
        self.community_dir(id).join("history-keys.bin")
    }
    pub fn community_key(&self, id: &CommunityId) -> PathBuf {
        self.community_dir(id).join("community.key")
    }
    pub fn events_log(&self, id: &CommunityId) -> PathBuf {
        self.community_dir(id).join("events.log")
    }
    /// Message content we have decrypted, so it survives MLS state loss. Sealed under the vault.
    pub fn content_log(&self, id: &CommunityId) -> PathBuf {
        self.community_dir(id).join("content.sealed")
    }
    /// The plaintext content log from before sealing. Migrated into `content_log` once, then removed.
    pub fn legacy_content_log(&self, id: &CommunityId) -> PathBuf {
        self.community_dir(id).join("content.log")
    }
    /// Every MLS commit this device applied, in epoch order, for catch-up.
    pub fn commit_log(&self, id: &CommunityId) -> PathBuf {
        self.community_dir(id).join("mls_commits.log")
    }
    /// Snapshot of the MLS client: storage values and the key package pool.
    pub fn mls_state(&self) -> PathBuf {
        self.root.join("mls.bin")
    }
    pub fn dms_dir(&self) -> PathBuf {
        self.root.join("dms")
    }
    /// Every direct message with `peer`, in either direction.
    pub fn dm_log(&self, peer: &RootId) -> PathBuf {
        self.dms_dir().join(format!("{peer}.log"))
    }
    pub fn outbox(&self) -> PathBuf {
        self.dms_dir().join("outbox.bin")
    }
    /// The root's guardian list, signed by the root key.
    pub fn guardians(&self) -> PathBuf {
        self.root.join("guardians.bin")
    }
    /// Devices of our root, as signed certs with their names.
    pub fn attachments(&self) -> PathBuf {
        self.root.join("attachments.bin")
    }
    pub fn downloads(&self) -> PathBuf {
        self.root.join("downloads")
    }
    pub fn devices(&self) -> PathBuf {
        self.root.join("devices.bin")
    }
    /// Notes, nicknames, pins and read markers. Never sent to a peer.
    pub fn local_state(&self) -> PathBuf {
        self.root.join("local.bin")
    }
    /// This device's own status. Its own file, so `local.bin` keeps its layout.
    pub fn own_status(&self) -> PathBuf {
        self.root.join("status.bin")
    }
}

fn encoding(e: postcard::Error) -> CoreError {
    CoreError::Encoding(e.to_string())
}

/// Appends one record to a log, creating the file and its directory when needed.
pub(crate) fn append_record<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    append_records(path, std::slice::from_ref(value))
}

/// Appends several records with one write and one sync. Does nothing for an empty slice.
pub(crate) fn append_records<T: Serialize>(path: &Path, values: &[T]) -> Result<()> {
    if values.is_empty() {
        return Ok(());
    }
    let framed = frame_records(values)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(&framed)?;
    file.sync_data()?;
    Ok(())
}

/// How long an events log write may wait in the page cache before it is synced. A crash of this process
/// loses nothing, since the write is already with the OS. A power loss can lose this window of events, and
/// peers send them again when we sync with them. MLS state that depends on an event is written only after
/// the event's log has been synced (see `write_mls_snapshot`).
pub(crate) const LOG_SYNC_EVERY: Duration = Duration::from_millis(250);

/// Logs written without a sync, waiting for the next group flush.
#[derive(Default)]
pub(crate) struct DirtyLogs {
    paths: Mutex<BTreeSet<PathBuf>>,
}

impl DirtyLogs {
    fn note(&self, path: &Path) {
        self.paths
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(path.to_path_buf());
    }

    /// Syncs every log written since the last flush, once each. A log that cannot be synced stays dirty
    /// for the next flush.
    pub(crate) fn flush(&self) {
        let paths = std::mem::take(&mut *self.paths.lock().unwrap_or_else(PoisonError::into_inner));
        for path in paths {
            let synced = OpenOptions::new()
                .append(true)
                .open(&path)
                .and_then(|file| file.sync_data());
            match synced {
                Ok(()) => {}
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "could not sync a log");
                    self.note(&path);
                }
            }
        }
    }
}

/// Like `append_records`, but without the sync. The log is synced by the next `DirtyLogs::flush`.
pub(crate) fn append_records_deferred<T: Serialize>(path: &Path, values: &[T], dirty: &DirtyLogs) -> Result<()> {
    if values.is_empty() {
        return Ok(());
    }
    let framed = frame_records(values)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(&framed)?;
    dirty.note(path);
    Ok(())
}

/// Syncs the deferred logs every `LOG_SYNC_EVERY`, until the engine is gone.
pub(crate) async fn log_flush_loop(weak: std::sync::Weak<crate::engine::Inner>) {
    loop {
        tokio::time::sleep(LOG_SYNC_EVERY).await;
        let Some(inner) = weak.upgrade() else {
            return;
        };
        let _ = tokio::task::spawn_blocking(move || inner.dirty_logs.flush()).await;
    }
}

/// Replaces every record of a log with `values`, atomically. Used when a stored record must lose
/// its content, such as a deleted message's text.
pub(crate) fn rewrite_records<T: Serialize>(path: &Path, values: &[T]) -> Result<()> {
    let framed = frame_records(values)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    write_bytes_atomic(path, &framed)
}

/// The length-prefixed bytes of each record, in order.
fn frame_records<T: Serialize>(values: &[T]) -> Result<Vec<u8>> {
    let mut framed = Vec::new();
    for value in values {
        let body = postcard::to_allocvec(value).map_err(encoding)?;
        let len = u32::try_from(body.len())
            .map_err(|_| CoreError::Encoding("record exceeds 4 GiB".into()))?;
        framed.extend_from_slice(&len.to_be_bytes());
        framed.extend_from_slice(&body);
    }
    Ok(framed)
}

/// Reads every record of a log that this build can decode. A record it cannot decode, as one a newer
/// build wrote, is logged and skipped, and its bytes stay in the file. A partly written tail is cut
/// as in `read_records`.
pub(crate) fn read_records_skipping<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    let frames = read_frames(path)?;
    Ok(frames
        .iter()
        .filter_map(|frame| match postcard::from_bytes(frame) {
            Ok(value) => Some(value),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skipped a record this build cannot decode");
                None
            }
        })
        .collect())
}

/// Reads every complete record from a log. A partly written tail is ignored and cut
/// from the file, so later appends stay aligned with the record framing.
pub(crate) fn read_records<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    read_frames(path)?
        .iter()
        .map(|frame| postcard::from_bytes(frame).map_err(encoding))
        .collect()
}

/// The raw bytes of each complete record in a log, cutting a partly written tail from the file.
fn read_frames(path: &Path) -> Result<Vec<Vec<u8>>> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    let mut pos = 0usize;
    while let Some(head) = bytes.get(pos..pos + 4) {
        let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let Some(end) = (pos + 4).checked_add(len).filter(|&end| end <= bytes.len()) else {
            break;
        };
        out.push(bytes[pos + 4..end].to_vec());
        pos = end;
    }
    if pos < bytes.len() {
        let file = OpenOptions::new().write(true).open(path)?;
        file.set_len(pos as u64)?;
        file.sync_all()?;
    }
    Ok(out)
}

/// Serialises `value` and replaces the file at `path` atomically.
pub(crate) fn write_bin<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = postcard::to_allocvec(value).map_err(encoding)?;
    write_bytes_atomic(path, &bytes)
}

/// Replaces the file at `path` with `bytes`. Writes a temp file, syncs it, then renames.
pub(crate) fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut file = fs::File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Reads a postcard `.bin` file. A missing file is `None`.
pub(crate) fn read_bin<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => postcard::from_bytes(&bytes).map(Some).map_err(encoding),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Replaces the file at `path` with `bytes`, through a temp file and a rename, without syncing.
/// A crash can lose the write, so only use this where losing the newest state is acceptable.
pub(crate) fn write_bytes_unsynced(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Label bound to every sealed content record. A record sealed for another file does not open here.
pub(crate) const CONTENT_AAD: &[u8] = b"dissent/content-log.sealed";

/// Subkey purpose for a founder's community signing key, and the label its seal is bound to.
const COMMUNITY_KEY_PURPOSE: &[u8] = b"dissent/community-key/v1";
const COMMUNITY_KEY_AAD: &[u8] = b"dissent/community-key.sealed";

/// The label a community key seal is bound to. It names the community, so a file moved to another
/// community's folder does not open.
pub(crate) fn community_key_aad(community: &CommunityId) -> Vec<u8> {
    [COMMUNITY_KEY_AAD, community.0.as_slice()].concat()
}

/// Seals a founder's community signing seed under `vault`.
pub(crate) fn write_community_key(path: &Path, vault: &Vault, community: &CommunityId, seed: &[u8; 32]) -> Result<()> {
    let sealed = vault
        .subkey(COMMUNITY_KEY_PURPOSE)
        .seal(&community_key_aad(community), seed);
    write_bytes_atomic(path, &sealed)
}

/// Reads a founder's community signing seed. `None` when there is no file.
///
/// A file that does not open under `vault` is refused, and never overwritten. The one exception is a
/// plaintext seed from before sealing, which is exactly 32 bytes. It is sealed in place on first read.
pub(crate) fn read_community_key(path: &Path, vault: &Vault, community: &CommunityId) -> Result<Option<[u8; 32]>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if let Ok(plain) = vault
        .subkey(COMMUNITY_KEY_PURPOSE)
        .open_sealed(&community_key_aad(community), &bytes)
    {
        let seed = <[u8; 32]>::try_from(plain.as_slice())
            .map_err(|_| CoreError::Invalid("community key has the wrong length".into()))?;
        return Ok(Some(seed));
    }
    let seed = <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| {
        CoreError::Invalid(format!("community key {} does not open under this vault", community))
    })?;
    write_community_key(path, vault, community, &seed)?;
    Ok(Some(seed))
}

/// The vault for one community's content log. Each log has its own subkey.
pub(crate) fn content_vault(vault: &Vault, community: &CommunityId) -> Vault {
    vault.subkey(&[b"dissent/content-log/v1".as_slice(), community.0.as_slice()].concat())
}

/// Appends sealed records. Same framing as `append_records`, with each record sealed.
pub(crate) fn append_sealed_records<T: Serialize>(
    path: &Path,
    vault: &Vault,
    values: &[T],
) -> Result<()> {
    if values.is_empty() {
        return Ok(());
    }
    let mut framed = Vec::new();
    for value in values {
        let body = postcard::to_allocvec(value).map_err(encoding)?;
        let sealed = vault.seal(CONTENT_AAD, &body);
        let len = u32::try_from(sealed.len())
            .map_err(|_| CoreError::Encoding("record exceeds 4 GiB".into()))?;
        framed.extend_from_slice(&len.to_be_bytes());
        framed.extend_from_slice(&sealed);
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(&framed)?;
    file.sync_data()?;
    Ok(())
}

/// Reads sealed records. A partly written tail is cut, as in `read_records`. A record that does not
/// open under `vault` is an error, not a skipped record.
pub(crate) fn read_sealed_records<T: DeserializeOwned>(path: &Path, vault: &Vault) -> Result<Vec<T>> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    let mut pos = 0usize;
    while let Some(head) = bytes.get(pos..pos + 4) {
        let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let Some(end) = (pos + 4).checked_add(len).filter(|&end| end <= bytes.len()) else {
            break;
        };
        let body = vault
            .open_sealed(CONTENT_AAD, &bytes[pos + 4..end])
            .map_err(|e| CoreError::Invalid(format!("content log does not open: {e}")))?;
        out.push(postcard::from_bytes(&body).map_err(encoding)?);
        pos = end;
    }
    if pos < bytes.len() {
        let file = OpenOptions::new().write(true).open(path)?;
        file.set_len(pos as u64)?;
        file.sync_all()?;
    }
    Ok(out)
}

/// Moves a plaintext log into its sealed file, once. A crash between the two steps repeats some
/// records on the next open. Replay keeps the newest record per key, so repeats are harmless.
pub(crate) fn migrate_plain_log<T: Serialize + DeserializeOwned>(
    plain: &Path,
    sealed: &Path,
    vault: &Vault,
) -> Result<()> {
    if !plain.exists() {
        return Ok(());
    }
    let records: Vec<T> = read_records(plain)?;
    append_sealed_records(sealed, vault, &records)?;
    fs::remove_file(plain)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn a_community_key_is_sealed_at_rest() {
        let dir = scratch("community-key-rest");
        let path = dir.join("community.key");
        let vault = Vault::from_passphrase("correct horse", &[5u8; 16]).unwrap();
        let seed = [7u8; 32];
        let id = dissent_identity::CommunityKey::from_seed(seed).id();

        write_community_key(&path, &vault, &id, &seed).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_ne!(bytes.len(), 32);
        assert!(!contains(&bytes, &seed));
        assert_eq!(read_community_key(&path, &vault, &id).unwrap(), Some(seed));
    }

    #[test]
    fn a_plaintext_community_key_is_sealed_on_first_read() {
        let dir = scratch("community-key-migrate");
        let path = dir.join("community.key");
        let vault = Vault::from_passphrase("correct horse", &[5u8; 16]).unwrap();
        let seed = [8u8; 32];
        let id = dissent_identity::CommunityKey::from_seed(seed).id();
        fs::write(&path, seed).unwrap();

        assert_eq!(read_community_key(&path, &vault, &id).unwrap(), Some(seed));
        let bytes = fs::read(&path).unwrap();
        assert_ne!(bytes.len(), 32);
        assert!(!contains(&bytes, &seed));
        // Once sealed, a second read opens the same seed and does not rewrite the file.
        assert_eq!(read_community_key(&path, &vault, &id).unwrap(), Some(seed));
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn a_community_key_that_is_not_one_plain_seed_is_never_overwritten() {
        let dir = scratch("community-key-odd");
        let path = dir.join("community.key");
        let vault = Vault::from_passphrase("correct horse", &[5u8; 16]).unwrap();
        let id = dissent_identity::CommunityKey::from_seed([9u8; 32]).id();
        fs::write(&path, [1u8; 31]).unwrap();
        assert!(read_community_key(&path, &vault, &id).is_err());
        assert_eq!(fs::read(&path).unwrap(), vec![1u8; 31]);
    }

    #[test]
    fn a_community_key_opens_only_under_its_own_vault_and_community() {
        let dir = scratch("community-key-wrong");
        let path = dir.join("community.key");
        let right = Vault::from_passphrase("correct horse", &[5u8; 16]).unwrap();
        let wrong = Vault::from_passphrase("battery staple", &[5u8; 16]).unwrap();
        let seed = [10u8; 32];
        let id = dissent_identity::CommunityKey::from_seed(seed).id();
        let other = dissent_identity::CommunityKey::from_seed([11u8; 32]).id();
        write_community_key(&path, &right, &id, &seed).unwrap();
        let before = fs::read(&path).unwrap();

        // A wrong vault is refused, and the file is left as it was.
        assert!(read_community_key(&path, &wrong, &id).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        // A file moved to another community's folder does not open.
        assert!(read_community_key(&path, &right, &other).is_err());
        assert_eq!(read_community_key(&path, &right, &id).unwrap(), Some(seed));
    }

    #[test]
    fn the_owner_still_signs_after_a_restart() {
        let dir = scratch("community-key-restart");
        let path = dir.join("community.key");
        let seed = [12u8; 32];
        let key = dissent_identity::CommunityKey::from_seed(seed);
        let id = key.id();
        write_community_key(&path, &Vault::from_passphrase("correct horse", &[6u8; 16]).unwrap(), &id, &seed).unwrap();

        // A restart derives the vault again from the passphrase and salt, then reads the key.
        let after = Vault::from_passphrase("correct horse", &[6u8; 16]).unwrap();
        let loaded = read_community_key(&path, &after, &id).unwrap().unwrap();
        let signer = dissent_identity::CommunityKey::from_seed(loaded);
        let sig = signer.sign(b"owner statement");
        assert!(id.verify(b"owner statement", &sig).is_ok());
        assert!(id.verify(b"another statement", &sig).is_err());
    }

    #[test]
    fn sealed_records_round_trip_and_hide_their_text() {
        let dir = scratch("sealed-log");
        let path = dir.join("content.sealed");
        let vault = Vault::ephemeral_for_tests();
        let key = content_vault(&vault, &CommunityId([3u8; 32]));
        append_sealed_records(&path, &key, &[("hello plaintext".to_string(), 1u32)]).unwrap();

        let back: Vec<(String, u32)> = read_sealed_records(&path, &key).unwrap();
        assert_eq!(back, vec![("hello plaintext".to_string(), 1)]);
        let raw = fs::read(&path).unwrap();
        assert!(!raw.windows(15).any(|w| w == b"hello plaintext"));

        // Another community's key does not open this log.
        let other = content_vault(&vault, &CommunityId([4u8; 32]));
        assert!(read_sealed_records::<(String, u32)>(&path, &other).is_err());
    }

    #[test]
    fn plaintext_log_migrates_into_the_sealed_file_once() {
        let dir = scratch("migrate-log");
        let plain = dir.join("content.log");
        let sealed = dir.join("content.sealed");
        append_records(&plain, &[("old text".to_string(), 7u32)]).unwrap();
        let vault = Vault::ephemeral_for_tests();
        let key = content_vault(&vault, &CommunityId([3u8; 32]));

        migrate_plain_log::<(String, u32)>(&plain, &sealed, &key).unwrap();
        assert!(!plain.exists());
        let back: Vec<(String, u32)> = read_sealed_records(&sealed, &key).unwrap();
        assert_eq!(back, vec![("old text".to_string(), 7)]);

        // Nothing left to migrate, so a second run changes nothing.
        migrate_plain_log::<(String, u32)>(&plain, &sealed, &key).unwrap();
        let again: Vec<(String, u32)> = read_sealed_records(&sealed, &key).unwrap();
        assert_eq!(again.len(), 1);
    }

    fn scratch(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "dissent-store-{name}-{}-{nanos}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn log_append_and_read_roundtrip() {
        let dir = scratch("roundtrip");
        let log = dir.join("events.log");
        for s in ["one", "two", "three"] {
            append_record(&log, &s.to_string()).unwrap();
        }
        let back: Vec<String> = read_records(&log).unwrap();
        assert_eq!(back, ["one", "two", "three"]);
        let missing: Vec<String> = read_records(&dir.join("nope.log")).unwrap();
        assert!(missing.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn torn_tail_is_ignored_and_cut() {
        let dir = scratch("torn");
        let log = dir.join("events.log");
        append_record(&log, &"kept".to_string()).unwrap();
        let good_len = fs::metadata(&log).unwrap().len();

        // A length header promising 20 bytes, with only 3 of them written.
        let mut f = OpenOptions::new().append(true).open(&log).unwrap();
        f.write_all(&[0, 0, 0, 20, 1, 2, 3]).unwrap();
        drop(f);

        let back: Vec<String> = read_records(&log).unwrap();
        assert_eq!(back, ["kept"]);
        assert_eq!(fs::metadata(&log).unwrap().len(), good_len);

        // Appends after the repair are still readable.
        append_record(&log, &"after".to_string()).unwrap();
        let back: Vec<String> = read_records(&log).unwrap();
        assert_eq!(back, ["kept", "after"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn partial_length_header_is_ignored() {
        let dir = scratch("header");
        let log = dir.join("events.log");
        append_record(&log, &7u32).unwrap();
        let mut f = OpenOptions::new().append(true).open(&log).unwrap();
        f.write_all(&[0, 0]).unwrap();
        drop(f);
        let back: Vec<u32> = read_records(&log).unwrap();
        assert_eq!(back, [7]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bin_replace_is_atomic_and_reads_back() {
        let dir = scratch("bin");
        let path = dir.join("profile.bin");
        assert_eq!(read_bin::<String>(&path).unwrap(), None);
        write_bin(&path, &"first".to_string()).unwrap();
        write_bin(&path, &"second".to_string()).unwrap();
        assert_eq!(
            read_bin::<String>(&path).unwrap().as_deref(),
            Some("second")
        );
        assert!(!dir.join("profile.tmp").exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod skipping_tests {
    use super::*;

    /// A record of the framing `append_records` writes: a big-endian length, then the bytes.
    fn frame(bytes: &[u8]) -> Vec<u8> {
        let mut out = (bytes.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(bytes);
        out
    }

    #[test]
    fn a_record_this_build_cannot_decode_is_skipped_and_kept_on_disk() {
        let dir = std::env::temp_dir().join(format!("dissent-skip-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("events.log");
        // Two good (u8, u8) records around one that is too short to decode as one.
        let mut bytes = frame(&[1, 2]);
        bytes.extend(frame(&[9]));
        bytes.extend(frame(&[3, 4]));
        fs::write(&path, &bytes).unwrap();

        let good: Vec<(u8, u8)> = read_records_skipping(&path).unwrap();
        assert_eq!(good, vec![(1, 2), (3, 4)]);
        // The skipped record is still in the file, so a later version can read it.
        assert_eq!(fs::read(&path).unwrap(), bytes);
        // The strict reader still refuses the file, as before.
        assert!(read_records::<(u8, u8)>(&path).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod group_commit_tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dissent-group-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_record_cut_mid_write_is_dropped_and_the_rest_loads() {
        let d = dir("cut");
        let path = d.join("events.log");
        let dirty = DirtyLogs::default();
        append_records_deferred(&path, &["one".to_string(), "two".to_string()], &dirty).unwrap();
        dirty.flush();
        // A crash partway through the last record: its last bytes never reached the disk.
        let len = fs::metadata(&path).unwrap().len();
        OpenOptions::new().write(true).open(&path).unwrap().set_len(len - 3).unwrap();
        assert_eq!(read_records::<String>(&path).unwrap(), vec!["one".to_string()]);
        // The next append lands after the cut, aligned with the framing.
        append_records_deferred(&path, &["three".to_string()], &dirty).unwrap();
        dirty.flush();
        assert_eq!(read_records::<String>(&path).unwrap(), vec!["one".to_string(), "three".to_string()]);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_deferred_write_reads_back_before_any_flush() {
        let d = dir("read");
        let path = d.join("events.log");
        let dirty = DirtyLogs::default();
        append_records_deferred(&path, &[7u32, 8u32], &dirty).unwrap();
        assert_eq!(read_records::<u32>(&path).unwrap(), vec![7, 8]);
        dirty.flush();
        let _ = fs::remove_dir_all(&d);
    }

    /// Timing for the events log: 6000 appends, each synced, against the same appends written without a
    /// sync and synced once per `LOG_SYNC_EVERY`. Run with `--ignored --nocapture`.
    #[test]
    #[ignore = "timing: run with --ignored --nocapture"]
    fn six_thousand_appends_synced_each_or_grouped() {
        let d = dir("timing");
        let records: Vec<String> = (0..6000).map(|i| format!("event {i} {}", "x".repeat(200))).collect();

        let start = std::time::Instant::now();
        for r in &records {
            append_records(&d.join("synced.log"), std::slice::from_ref(r)).unwrap();
        }
        let synced = start.elapsed();

        let dirty = DirtyLogs::default();
        let start = std::time::Instant::now();
        let mut since = start;
        for r in &records {
            append_records_deferred(&d.join("grouped.log"), std::slice::from_ref(r), &dirty).unwrap();
            if since.elapsed() >= LOG_SYNC_EVERY {
                dirty.flush();
                since = std::time::Instant::now();
            }
        }
        dirty.flush();
        let grouped = start.elapsed();

        println!("6000 appends: synced each {synced:?}, grouped {grouped:?}");
        assert_eq!(read_records::<String>(&d.join("grouped.log")).unwrap().len(), 6000);
        let _ = fs::remove_dir_all(&d);
    }
}
