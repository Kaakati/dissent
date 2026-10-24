//! Disk layout and file helpers.
//!
//! Logs are repeated `[u32 BE length][postcard record]`, appended in one write.
//! `.bin` files are replaced whole, through a temp file and a rename.

use crate::error::{CoreError, Result};
use dissent_identity::{CommunityId, RootId, Vault};
use serde::{Serialize, de::DeserializeOwned};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

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
    /// Contact requests, accepts we owe, declines and the DM policy.
    pub fn contact_book(&self) -> PathBuf {
        self.root.join("contact_book.bin")
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
    let mut framed = Vec::new();
    for value in values {
        let body = postcard::to_allocvec(value).map_err(encoding)?;
        let len = u32::try_from(body.len())
            .map_err(|_| CoreError::Encoding("record exceeds 4 GiB".into()))?;
        framed.extend_from_slice(&len.to_be_bytes());
        framed.extend_from_slice(&body);
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(&framed)?;
    file.sync_data()?;
    Ok(())
}

/// Reads every complete record from a log. A partly written tail is ignored and cut
/// from the file, so later appends stay aligned with the record framing.
pub(crate) fn read_records<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
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
        out.push(postcard::from_bytes(&bytes[pos + 4..end]).map_err(encoding)?);
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
const CONTENT_AAD: &[u8] = b"dissent/content-log.sealed";

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
