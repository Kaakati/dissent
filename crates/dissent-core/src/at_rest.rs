//! The profile's files at rest, sealed under the vault. A `.bin` file is one sealed value; a log is a header and
//! sealed records. Each is bound to its place in the profile, so a file copied over another does not open. Files
//! from before sealing still read; `migrate` seals them on the first start, and every write is sealed. On Unix the
//! profile is kept to its owner: folders 0700, files 0600. See `docs/design/06-threat-model.md`, "Data at rest".

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use dissent_identity::Vault;
use serde::{de::DeserializeOwned, Serialize};

use crate::error::{CoreError, Result};
use crate::store::Paths;

/// The first bytes of a sealed file: one sealed value follows.
pub(crate) const FILE_MAGIC: &[u8; 4] = b"DSF\x01";
/// The first bytes of a sealed log: length-prefixed sealed records follow.
pub(crate) const LOG_MAGIC: &[u8; 4] = b"DSL\x01";
/// The subkey every sealed file and log is sealed under.
pub(crate) const STORE_PURPOSE: &[u8] = b"dissent/store/v1";
const AAD_PREFIX: &[u8] = b"dissent/store-file/v1/";

fn encoding(e: postcard::Error) -> CoreError {
    CoreError::Encoding(e.to_string())
}

/// Creates `dir` and its parents. On Unix a folder this makes is its owner's only.
pub(crate) fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)
    }
}

/// Options for a new file that only its owner may read, on Unix.
fn private_file() -> OpenOptions {
    #[allow(unused_mut)]
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

/// Replaces `path` with `bytes`: a temp file beside it, synced, then renamed over it. The temp file holds the same
/// bytes, so a sealed write never leaves a plaintext copy.
fn replace(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        create_private_dir(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut file = private_file().write(true).create(true).truncate(true).open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, path)?;
    Ok(())
}

/// The frames of a log body: `[u32 BE length][bytes]` each. Returns them and where the last complete one ends.
fn frames(bytes: &[u8]) -> (Vec<&[u8]>, usize) {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while let Some(head) = bytes.get(pos..pos + 4) {
        let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let Some(end) = (pos + 4).checked_add(len).filter(|&end| end <= bytes.len()) else {
            break;
        };
        out.push(&bytes[pos + 4..end]);
        pos = end;
    }
    (out, pos)
}

fn frame(out: &mut Vec<u8>, body: &[u8]) -> Result<()> {
    let len = u32::try_from(body.len()).map_err(|_| CoreError::Encoding("record exceeds 4 GiB".into()))?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(body);
    Ok(())
}

fn read_or_none(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

impl Paths {
    /// What a file's seal is bound to: its path inside the profile.
    fn aad(&self, path: &Path) -> Vec<u8> {
        let rel = path.strip_prefix(self.root()).unwrap_or(path);
        let name: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        [AAD_PREFIX, name.join("/").as_bytes()].concat()
    }

    fn store_vault(&self) -> &Vault {
        self.store_seal()
    }

    /// Seals `bytes` as the file at `path`.
    fn seal_file(&self, path: &Path, bytes: &[u8]) -> Vec<u8> {
        [FILE_MAGIC.as_slice(), &self.store_vault().seal(&self.aad(path), bytes)].concat()
    }

    /// The bytes of a sealed file, or of a file from before sealing. A sealed file that does not open is an error,
    /// so a file is never taken for plaintext by mistake.
    fn open_file(&self, path: &Path, bytes: Vec<u8>) -> Result<Vec<u8>> {
        match bytes.strip_prefix(FILE_MAGIC.as_slice()) {
            Some(sealed) => self
                .store_vault()
                .open_sealed(&self.aad(path), sealed)
                .map_err(|e| CoreError::Invalid(format!("{} does not open: {e}", path.display()))),
            None => Ok(bytes),
        }
    }

    /// Reads a file written by `write_bytes`, or a plaintext file from before sealing. A missing file is `None`.
    pub(crate) fn read_bytes(&self, path: &Path) -> Result<Option<Vec<u8>>> {
        read_or_none(path)?.map(|bytes| self.open_file(path, bytes)).transpose()
    }

    /// Replaces the file at `path` with `bytes`, sealed.
    pub(crate) fn write_bytes(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        replace(path, &self.seal_file(path, bytes))
    }

    /// Reads a postcard value written by `write_bin`, or a plaintext `.bin` from before sealing. A missing file is `None`.
    pub(crate) fn read_bin<T: DeserializeOwned>(&self, path: &Path) -> Result<Option<T>> {
        match self.read_bytes(path)? {
            Some(bytes) => postcard::from_bytes(&bytes).map(Some).map_err(encoding),
            None => Ok(None),
        }
    }

    /// Serialises `value` and replaces the file at `path` with it, sealed.
    pub(crate) fn write_bin<T: Serialize>(&self, path: &Path, value: &T) -> Result<()> {
        let bytes = postcard::to_allocvec(value).map_err(encoding)?;
        self.write_bytes(path, &bytes)
    }

    /// Every complete record body of a log, opened. A sealed log is the header and sealed records; a log from before
    /// sealing is plain records. A partly written tail is cut from the file, so later appends stay aligned.
    fn log_bodies(&self, path: &Path) -> Result<Vec<Vec<u8>>> {
        let Some(bytes) = read_or_none(path)? else {
            return Ok(Vec::new());
        };
        let (sealed, body, start) = match bytes.strip_prefix(LOG_MAGIC.as_slice()) {
            Some(rest) => (true, rest, LOG_MAGIC.len()),
            None => (false, bytes.as_slice(), 0),
        };
        let (found, end) = frames(body);
        let aad = self.aad(path);
        let out = found
            .into_iter()
            .map(|frame| {
                if sealed {
                    self.store_vault()
                        .open_sealed(&aad, frame)
                        .map_err(|e| CoreError::Invalid(format!("{} does not open: {e}", path.display())))
                } else {
                    Ok(frame.to_vec())
                }
            })
            .collect::<Result<Vec<_>>>()?;
        if start + end < bytes.len() {
            let file = OpenOptions::new().write(true).open(path)?;
            file.set_len((start + end) as u64)?;
            file.sync_all()?;
        }
        Ok(out)
    }

    /// The sealed log of `bodies`, header included.
    fn sealed_log(&self, path: &Path, bodies: &[Vec<u8>]) -> Result<Vec<u8>> {
        let aad = self.aad(path);
        let mut out = LOG_MAGIC.to_vec();
        for body in bodies {
            frame(&mut out, &self.store_vault().seal(&aad, body))?;
        }
        Ok(out)
    }

    /// Reads every record of a log, sealed or from before sealing.
    pub(crate) fn read_records<T: DeserializeOwned>(&self, path: &Path) -> Result<Vec<T>> {
        self.log_bodies(path)?
            .iter()
            .map(|body| postcard::from_bytes(body).map_err(encoding))
            .collect()
    }

    /// Appends sealed records with one write and one sync. A log from before sealing is sealed first, whole.
    pub(crate) fn append_records<T: Serialize>(&self, path: &Path, values: &[T]) -> Result<()> {
        if values.is_empty() {
            return Ok(());
        }
        let bodies = values
            .iter()
            .map(|v| postcard::to_allocvec(v).map_err(encoding))
            .collect::<Result<Vec<_>>>()?;
        let head = read_or_none(path)?;
        match head {
            Some(bytes) if !bytes.is_empty() && !bytes.starts_with(LOG_MAGIC.as_slice()) => {
                let mut all = self.log_bodies(path)?;
                all.extend(bodies);
                replace(path, &self.sealed_log(path, &all)?)
            }
            Some(bytes) if !bytes.is_empty() => {
                let aad = self.aad(path);
                let mut framed = Vec::new();
                for body in &bodies {
                    frame(&mut framed, &self.store_vault().seal(&aad, body))?;
                }
                let mut file = OpenOptions::new().append(true).open(path)?;
                file.write_all(&framed)?;
                file.sync_data()?;
                Ok(())
            }
            _ => replace(path, &self.sealed_log(path, &bodies)?),
        }
    }

    /// Appends one sealed record.
    pub(crate) fn append_record<T: Serialize>(&self, path: &Path, value: &T) -> Result<()> {
        self.append_records(path, std::slice::from_ref(value))
    }

    /// Replaces every record of a log with `values`, sealed and atomic.
    pub(crate) fn rewrite_records<T: Serialize>(&self, path: &Path, values: &[T]) -> Result<()> {
        let bodies = values
            .iter()
            .map(|v| postcard::to_allocvec(v).map_err(encoding))
            .collect::<Result<Vec<_>>>()?;
        replace(path, &self.sealed_log(path, &bodies)?)
    }
}

// ---- migration of a profile from before sealing ----

/// Files and folders that stay as they are: sealed in their own way, public data (the events log and the MLS commit
/// log hold only what every member's peers relay), the blob store (ciphertext), and the user's downloads.
const KEEP_NAMES: &[&str] = &[
    "mls.bin",
    "avatar.bin",
    "peer_avatars.bin",
    "banners.bin",
    "events.log",
    "mls_commits.log",
    "content.sealed",
    "content.log",
    "community.key",
    "epoch-keys.bin",
    "history-keys.bin",
    "private-keys.bin",
    MIGRATED_MARK,
];
const KEEP_DIRS: &[&str] = &["files", "downloads", "staging", BACKUP_DIR];
/// Written once every file is sealed, so a start after that skips the walk.
pub(crate) const MIGRATED_MARK: &str = "store-sealed.v1";
/// While a migration runs, the files it is about to seal are copied here; it is removed once all are sealed.
pub(crate) const BACKUP_DIR: &str = "unsealed-backup";

fn is_log(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()), Some("log" | "acts"))
}

/// The files under `dir` a migration seals.
fn plaintext_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            if !KEEP_DIRS.contains(&name.as_str()) {
                plaintext_files(&path, out)?;
            }
            continue;
        }
        if !kind.is_file() || KEEP_NAMES.contains(&name.as_str()) || name.ends_with(".tmp") {
            continue;
        }
        let head = {
            let mut head = [0u8; 4];
            let mut file = fs::File::open(&path)?;
            let n = std::io::Read::read(&mut file, &mut head)?;
            head[..n].to_vec()
        };
        if head == FILE_MAGIC.as_slice() || head == LOG_MAGIC.as_slice() {
            continue;
        }
        out.push(path);
    }
    Ok(())
}

impl Paths {
    /// Seals every file of a profile from before sealing, once. Each file is replaced atomically, and a copy of each
    /// waits in `unsealed-backup/` until all are sealed, then the copies are removed. A start interrupted half way runs
    /// again: sealed files are skipped, and a file that reads either way is never lost. On Unix the profile's folders
    /// become 0700 and its files 0600 as well.
    pub(crate) fn migrate(&self) -> Result<usize> {
        let root = self.root().to_path_buf();
        harden(&root);
        if root.join(MIGRATED_MARK).exists() {
            return Ok(0);
        }
        let mut files = Vec::new();
        plaintext_files(&root, &mut files)?;
        if !files.is_empty() {
            let backup = root.join(BACKUP_DIR);
            for file in &files {
                let rel = file.strip_prefix(&root).unwrap_or(file);
                let copy = backup.join(rel);
                if let Some(dir) = copy.parent() {
                    create_private_dir(dir)?;
                }
                // A backup left by an interrupted run is the older copy, and is kept.
                if !copy.exists() {
                    fs::copy(file, &copy)?;
                }
            }
            for file in &files {
                if is_log(file) {
                    let bodies = self.log_bodies(file)?;
                    replace(file, &self.sealed_log(file, &bodies)?)?;
                } else {
                    let bytes = fs::read(file)?;
                    self.write_bytes(file, &bytes)?;
                }
            }
        }
        replace(&root.join(MIGRATED_MARK), b"1")?;
        let backup = root.join(BACKUP_DIR);
        if backup.exists() {
            fs::remove_dir_all(&backup)?;
        }
        if !files.is_empty() {
            tracing::info!(files = files.len(), "sealed the profile's files from before sealing");
        }
        Ok(files.len())
    }
}

/// Seals every sealed file and log of the profile at `root` again, from `from`'s seal to `to`'s, in place. Used by a
/// profile copy under a new vault. A file that does not open under `from` is an error, so a damaged or foreign file is
/// never sealed again as if it were good. Files still in plaintext are sealed under `to` after.
pub(crate) fn reseal_tree(root: &Path, from: &Vault, to: &Vault) -> Result<()> {
    let old = Paths::sealed_by(root.to_path_buf(), from);
    let new = Paths::sealed_by(root.to_path_buf(), to);
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
        let Ok(entries) = fs::read_dir(dir) else {
            return Ok(());
        };
        for entry in entries.flatten() {
            let kind = entry.file_type()?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if kind.is_dir() && !KEEP_DIRS.contains(&name.as_str()) {
                walk(&entry.path(), out)?;
            } else if kind.is_file() {
                out.push(entry.path());
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(root, &mut files)?;
    for file in files {
        let mut head = [0u8; 4];
        let n = std::io::Read::read(&mut fs::File::open(&file)?, &mut head)?;
        if &head[..n] == FILE_MAGIC.as_slice() {
            let bytes = old.read_bytes(&file)?.unwrap_or_default();
            new.write_bytes(&file, &bytes)?;
        } else if &head[..n] == LOG_MAGIC.as_slice() {
            let bodies = old.log_bodies(&file)?;
            replace(&file, &new.sealed_log(&file, &bodies)?)?;
        }
    }
    // A copy of a profile from before sealing comes out sealed too.
    new.migrate()?;
    Ok(())
}

/// On Unix, makes every folder under `root` 0700 and every file 0600. Elsewhere the profile folder's ACL already
/// limits it to the user.
pub fn harden(root: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fn walk(dir: &Path) {
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
            let Ok(entries) = fs::read_dir(dir) else { return };
            for entry in entries.flatten() {
                match entry.file_type() {
                    Ok(kind) if kind.is_dir() => walk(&entry.path()),
                    Ok(kind) if kind.is_file() => {
                        let _ = fs::set_permissions(entry.path(), fs::Permissions::from_mode(0o600));
                    }
                    _ => {}
                }
            }
        }
        walk(root);
    }
    #[cfg(not(unix))]
    {
        let _ = root;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dissent-at-rest-{name}-{}-{}", std::process::id(), crate::engine::now_ms()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn paths(dir: &Path) -> Paths {
        Paths::new(dir.to_path_buf(), Arc::new(Vault::from_passphrase("at rest", &[3u8; 16]).unwrap()))
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn a_bin_file_is_sealed_and_reads_back() {
        let dir = scratch("bin");
        let p = paths(&dir);
        let path = dir.join("bookmarks.bin");
        p.write_bin(&path, &vec!["a secret bookmark".to_string()]).unwrap();
        let raw = fs::read(&path).unwrap();
        assert!(raw.starts_with(FILE_MAGIC));
        assert!(!contains(&raw, b"a secret bookmark"));
        assert_eq!(p.read_bin::<Vec<String>>(&path).unwrap(), Some(vec!["a secret bookmark".to_string()]));
        assert!(!dir.join("bookmarks.tmp").exists(), "no temp file is left");
    }

    #[test]
    fn a_plaintext_bin_still_reads_and_the_next_write_seals_it() {
        let dir = scratch("legacy-bin");
        let p = paths(&dir);
        let path = dir.join("local.bin");
        fs::write(&path, postcard::to_allocvec(&("note", 7u32)).unwrap()).unwrap();
        assert_eq!(p.read_bin::<(String, u32)>(&path).unwrap(), Some(("note".into(), 7)));
        p.write_bin(&path, &("note", 8u32)).unwrap();
        assert!(fs::read(&path).unwrap().starts_with(FILE_MAGIC));
    }

    #[test]
    fn a_sealed_file_does_not_open_under_another_key_or_in_another_place() {
        let dir = scratch("wrong");
        let p = paths(&dir);
        let path = dir.join("contacts.bin");
        p.write_bin(&path, &1u32).unwrap();
        let other = Paths::new(dir.clone(), Arc::new(Vault::from_passphrase("other", &[3u8; 16]).unwrap()));
        assert!(other.read_bin::<u32>(&path).is_err(), "another key does not open it");
        let moved = dir.join("addresses.bin");
        fs::copy(&path, &moved).unwrap();
        assert!(p.read_bin::<u32>(&moved).is_err(), "a file copied over another does not open");
    }

    #[test]
    fn a_log_is_sealed_appends_and_reads_back() {
        let dir = scratch("log");
        let p = paths(&dir);
        let path = dir.join("dms").join("peer.log");
        p.append_record(&path, &"first secret".to_string()).unwrap();
        p.append_records(&path, &["second".to_string(), "third".to_string()]).unwrap();
        let raw = fs::read(&path).unwrap();
        assert!(raw.starts_with(LOG_MAGIC));
        assert!(!contains(&raw, b"first secret"));
        assert_eq!(p.read_records::<String>(&path).unwrap(), ["first secret", "second", "third"]);
        p.rewrite_records(&path, &["only".to_string()]).unwrap();
        assert_eq!(p.read_records::<String>(&path).unwrap(), ["only"]);
    }

    #[test]
    fn a_plaintext_log_reads_and_is_sealed_on_the_next_append() {
        let dir = scratch("legacy-log");
        let p = paths(&dir);
        let path = dir.join("conv.log");
        crate::store::append_records_plain(&path, &["old secret".to_string()]).unwrap();
        assert_eq!(p.read_records::<String>(&path).unwrap(), ["old secret"]);
        p.append_record(&path, &"new".to_string()).unwrap();
        let raw = fs::read(&path).unwrap();
        assert!(raw.starts_with(LOG_MAGIC) && !contains(&raw, b"old secret"));
        assert_eq!(p.read_records::<String>(&path).unwrap(), ["old secret", "new"]);
    }

    #[test]
    fn a_torn_tail_of_a_sealed_log_is_cut() {
        let dir = scratch("torn");
        let p = paths(&dir);
        let path = dir.join("x.log");
        p.append_records(&path, &[1u32, 2u32]).unwrap();
        let mut raw = fs::read(&path).unwrap();
        let full = raw.len();
        raw.extend_from_slice(&[0, 0, 0, 40, 1, 2]);
        fs::write(&path, &raw).unwrap();
        assert_eq!(p.read_records::<u32>(&path).unwrap(), [1, 2]);
        assert_eq!(fs::read(&path).unwrap().len(), full);
        p.append_record(&path, &3u32).unwrap();
        assert_eq!(p.read_records::<u32>(&path).unwrap(), [1, 2, 3]);
    }

    #[test]
    fn migration_seals_every_plaintext_file_once_and_leaves_the_others() {
        let dir = scratch("migrate");
        let p = paths(&dir);
        fs::write(dir.join("bookmarks.bin"), postcard::to_allocvec(&"secret bookmark").unwrap()).unwrap();
        crate::store::append_records_plain(&dir.join("dms").join("a.log"), &["secret dm".to_string()]).unwrap();
        fs::create_dir_all(dir.join("communities").join("c")).unwrap();
        fs::write(dir.join("communities").join("c").join("events.log"), b"public events").unwrap();
        fs::create_dir_all(dir.join("thumbnails")).unwrap();
        fs::write(dir.join("thumbnails").join("t.jpg"), b"secret picture").unwrap();
        fs::create_dir_all(dir.join("files")).unwrap();
        fs::write(dir.join("files").join("blobs.db"), b"blob store").unwrap();
        assert_eq!(p.migrate().unwrap(), 3);
        for (file, secret) in [("bookmarks.bin", "secret bookmark"), ("dms/a.log", "secret dm"), ("thumbnails/t.jpg", "secret picture")] {
            assert!(!contains(&fs::read(dir.join(file)).unwrap(), secret.as_bytes()), "{file} is sealed");
        }
        assert_eq!(p.read_bin::<String>(&dir.join("bookmarks.bin")).unwrap().as_deref(), Some("secret bookmark"));
        assert_eq!(p.read_records::<String>(&dir.join("dms").join("a.log")).unwrap(), ["secret dm"]);
        assert_eq!(p.read_bytes(&dir.join("thumbnails").join("t.jpg")).unwrap().unwrap(), b"secret picture");
        assert_eq!(fs::read(dir.join("communities").join("c").join("events.log")).unwrap(), b"public events");
        assert_eq!(fs::read(dir.join("files").join("blobs.db")).unwrap(), b"blob store");
        assert!(!dir.join(BACKUP_DIR).exists(), "the backup goes once every file is sealed");
        assert!(dir.join(MIGRATED_MARK).exists());
        assert_eq!(p.migrate().unwrap(), 0, "a second start does nothing");
    }

    #[test]
    fn an_interrupted_migration_keeps_its_backup_and_finishes_on_the_next_start() {
        let dir = scratch("interrupted");
        let p = paths(&dir);
        fs::write(dir.join("local.bin"), b"secret notes").unwrap();
        // A run that copied the file but stopped before sealing it.
        fs::create_dir_all(dir.join(BACKUP_DIR)).unwrap();
        fs::write(dir.join(BACKUP_DIR).join("local.bin"), b"secret notes").unwrap();
        assert_eq!(p.migrate().unwrap(), 1);
        assert_eq!(p.read_bytes(&dir.join("local.bin")).unwrap().unwrap(), b"secret notes");
        assert!(!dir.join(BACKUP_DIR).exists());
    }

    #[test]
    fn a_copy_under_a_new_vault_reseals_every_store_file() {
        let dir = scratch("reseal");
        let old = Vault::from_passphrase("old", &[3u8; 16]).unwrap();
        let new = Vault::from_passphrase("new", &[3u8; 16]).unwrap();
        let before = Paths::sealed_by(dir.clone(), &old);
        before.write_bin(&dir.join("contacts.bin"), &"a contact".to_string()).unwrap();
        before.append_record(&dir.join("dms").join("p.log"), &"a dm".to_string()).unwrap();
        fs::write(dir.join("local.bin"), postcard::to_allocvec(&"plain note").unwrap()).unwrap();
        reseal_tree(&dir, &old, &new).unwrap();
        let after = Paths::sealed_by(dir.clone(), &new);
        assert_eq!(after.read_bin::<String>(&dir.join("contacts.bin")).unwrap().as_deref(), Some("a contact"));
        assert_eq!(after.read_records::<String>(&dir.join("dms").join("p.log")).unwrap(), ["a dm"]);
        assert!(fs::read(dir.join("local.bin")).unwrap().starts_with(FILE_MAGIC), "a plaintext file is sealed on the way");
        assert_eq!(after.read_bin::<String>(&dir.join("local.bin")).unwrap().as_deref(), Some("plain note"));
        assert!(before.read_bin::<String>(&dir.join("contacts.bin")).is_err(), "the old vault no longer opens it");
    }

    #[cfg(unix)]
    #[test]
    fn the_profile_is_its_owners_only_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("modes");
        let p = paths(&dir);
        fs::write(dir.join("old.bin"), b"x").unwrap();
        fs::set_permissions(dir.join("old.bin"), fs::Permissions::from_mode(0o644)).unwrap();
        p.migrate().unwrap();
        p.write_bin(&dir.join("sub").join("new.bin"), &1u8).unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join("old.bin")), 0o600);
        assert_eq!(mode(&dir.join("sub")), 0o700);
        assert_eq!(mode(&dir.join("sub").join("new.bin")), 0o600);
    }
}
