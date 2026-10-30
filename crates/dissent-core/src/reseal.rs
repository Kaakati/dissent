//! Copies a profile folder under a new vault. Every vault-sealed file is opened with the old vault and
//! sealed again under the new one. The copy is checked before it is put in place, and the source is only
//! read. See [`copy_profile`].
//!
//! Sealed files: `identity.bin`, `profiles/<id>/mls.bin`, `avatar.bin`, `peer_avatars.bin`, `banners.bin`, each
//! community's `content.sealed` log, `community.key`, `epoch-keys.bin`, `private-keys.bin` and `history-keys.bin`, and every
//! other store file and log, sealed under the store subkey (`at_rest`). Only the events and commit logs and the blob store
//! are not sealed: they hold what peers relay anyway.

use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use dissent_identity::{keystore, CommunityId, DeviceId, Vault};

use crate::avatars::{own_vault, peers_vault, FILE_AAD};
use crate::error::{CoreError, Result};
use crate::sealed::mls_err;
use crate::store::{content_vault, read_community_key, write_bytes_atomic, write_community_key, CONTENT_AAD};
use dissent_mls::MlsClient;

const IDENTITY_FILE: &str = "identity.bin";
const MLS_FILE: &str = "mls.bin";
const AVATAR_FILE: &str = "avatar.bin";
const PEER_AVATARS_FILE: &str = "peer_avatars.bin";
const BANNERS_FILE: &str = "banners.bin";
const CONTENT_FILE: &str = "content.sealed";
const COMMUNITY_KEY_FILE: &str = "community.key";
/// The lock is per process, so a copy never carries it.
const LOCK_FILE: &str = ".lock";

/// Copies the profile folder `source` to `dest`, sealing it under `to`. `from` opens `source`. `dest` must
/// not exist. `prepare` runs on the staged copy before it is put in place, so the caller can add a file
/// such as a salt. Nothing appears at `dest` unless every check passes.
pub fn copy_profile(
    source: &Path,
    from: &Vault,
    dest: &Path,
    to: &Vault,
    prepare: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    if dest.exists() {
        return Err(CoreError::Invalid("the destination already exists".into()));
    }
    let staging = staging_path(dest)?;
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    match stage(source, from, &staging, to, prepare) {
        Ok(()) => {
            fs::rename(&staging, dest)?;
            Ok(())
        }
        Err(e) => {
            let _ = fs::remove_dir_all(&staging);
            Err(e)
        }
    }
}

fn staging_path(dest: &Path) -> Result<PathBuf> {
    let name = dest
        .file_name()
        .ok_or_else(|| CoreError::Invalid("the destination has no name".into()))?;
    Ok(dest.with_file_name(format!("{}.copying", name.to_string_lossy())))
}

fn stage(source: &Path, from: &Vault, staging: &Path, to: &Vault, prepare: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
    copy_tree(source, staging)?;
    let identity_path = staging.join(IDENTITY_FILE);
    let identity = keystore::load(&identity_path, from)?;
    keystore::save(&identity, &identity_path, to)?;
    for profile in subdirs(&staging.join("profiles"))? {
        reseal_profile(&profile, from, to)?;
    }
    prepare(staging)?;
    verify(staging, to, identity.device_id())
}

/// Copies every file and folder under `src` except the lock.
fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == LOCK_FILE {
            continue;
        }
        let target = dst.join(&name);
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn subdirs(dir: &Path) -> Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            out.push(entry.path());
        }
    }
    Ok(out)
}

/// Community folders in one profile, with the id each one is named for.
fn community_dirs(profile: &Path) -> Result<Vec<(CommunityId, PathBuf)>> {
    let mut out = Vec::new();
    for dir in subdirs(&profile.join("communities"))? {
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        // Folders that are not named for a community id are not ours to touch.
        if let Ok(id) = CommunityId::from_str(&name) {
            out.push((id, dir));
        }
    }
    Ok(out)
}

fn reseal_profile(profile: &Path, from: &Vault, to: &Vault) -> Result<()> {
    let mls = profile.join(MLS_FILE);
    if mls.exists() {
        let bytes = fs::read(&mls)?;
        let moved = if MlsClient::is_legacy_snapshot(&bytes) {
            MlsClient::migrate_legacy(&bytes, to).map_err(mls_err)?
        } else {
            MlsClient::reseal_snapshot(&bytes, from, to).map_err(mls_err)?
        };
        write_bytes_atomic(&mls, &moved)?;
    }
    reseal_file(&profile.join(AVATAR_FILE), &own_vault(from), &own_vault(to))?;
    reseal_file(&profile.join(PEER_AVATARS_FILE), &peers_vault(from), &peers_vault(to))?;
    reseal_file(&profile.join(BANNERS_FILE), &peers_vault(from), &peers_vault(to))?;
    for (id, dir) in community_dirs(profile)? {
        reseal_records(&dir.join(CONTENT_FILE), &content_vault(from, &id), &content_vault(to, &id))?;
        reseal_community_key(&dir.join(COMMUNITY_KEY_FILE), from, to, &id)?;
        crate::epoch_keys::reseal(&dir.join("epoch-keys.bin"), from, to, &id)?;
        crate::private_keys::reseal(&dir.join("private-keys.bin"), from, to, &id)?;
        crate::history::reseal(&dir.join("history-keys.bin"), from, to, &id)?;
    }
    // Everything else in the profile is sealed under the store subkey (`at_rest`).
    crate::at_rest::reseal_tree(profile, from, to)
}

/// Re-seals one file written by `write_sealed`. A file that does not open under `old` is an error, so a
/// damaged or foreign file is never sealed again as if it were good.
fn reseal_file(path: &Path, old: &Vault, new: &Vault) -> Result<()> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let plain = old
        .open_sealed(FILE_AAD, &bytes)
        .map_err(|e| CoreError::Invalid(format!("{} does not open here: {e}", display_name(path))))?;
    write_bytes_atomic(path, &new.seal(FILE_AAD, &plain))
}

/// Re-seals a founder's community key. The seed is read under `from` and sealed under `to`.
fn reseal_community_key(path: &Path, from: &Vault, to: &Vault, community: &CommunityId) -> Result<()> {
    if let Some(seed) = read_community_key(path, from, community)? {
        write_community_key(path, to, community, &seed)?;
    }
    Ok(())
}

/// Re-seals a record log in the framing of `append_sealed_records`. A partly written tail is dropped, as
/// the reader drops it.
fn reseal_records(path: &Path, old: &Vault, new: &Vault) -> Result<()> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::with_capacity(bytes.len());
    for sealed in record_frames(&bytes) {
        let plain = old
            .open_sealed(CONTENT_AAD, sealed)
            .map_err(|e| CoreError::Invalid(format!("{} does not open here: {e}", display_name(path))))?;
        let again = new.seal(CONTENT_AAD, &plain);
        let len = u32::try_from(again.len()).map_err(|_| CoreError::Encoding("record exceeds 4 GiB".into()))?;
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(&again);
    }
    write_bytes_atomic(path, &out)
}

/// The sealed body of each complete record in a log, in order.
fn record_frames(bytes: &[u8]) -> Vec<&[u8]> {
    let mut frames = Vec::new();
    let mut pos = 0usize;
    while let Some(head) = bytes.get(pos..pos + 4) {
        let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let Some(end) = (pos + 4).checked_add(len).filter(|&end| end <= bytes.len()) else {
            break;
        };
        frames.push(&bytes[pos + 4..end]);
        pos = end;
    }
    frames
}

/// Opens every sealed file in the staged copy under `to`, and checks that the identity is the one that was
/// copied.
fn verify(staging: &Path, to: &Vault, device: DeviceId) -> Result<()> {
    let identity = keystore::load(&staging.join(IDENTITY_FILE), to)?;
    if identity.device_id() != device {
        return Err(CoreError::Invalid("the copied identity does not match the original".into()));
    }
    for profile in subdirs(&staging.join("profiles"))? {
        let mls = profile.join(MLS_FILE);
        if mls.exists() {
            MlsClient::unseal_snapshot(&fs::read(&mls)?, to).map_err(mls_err)?;
        }
        verify_file(&profile.join(AVATAR_FILE), &own_vault(to), FILE_AAD)?;
        verify_file(&profile.join(PEER_AVATARS_FILE), &peers_vault(to), FILE_AAD)?;
        verify_file(&profile.join(BANNERS_FILE), &peers_vault(to), FILE_AAD)?;
        for (id, dir) in community_dirs(&profile)? {
            // A plain 32-byte key here would mean the copy was not sealed. Reading it would migrate it silently, so check the length first.
            let key_file = dir.join(COMMUNITY_KEY_FILE);
            if key_file.exists() {
                if fs::read(&key_file)?.len() == 32 {
                    return Err(CoreError::Invalid("copied community key is not sealed".into()));
                }
                read_community_key(&key_file, to, &id)?
                    .ok_or_else(|| CoreError::Invalid("copied community key is missing".into()))?;
            }
            let path = dir.join(CONTENT_FILE);
            if !path.exists() {
                continue;
            }
            let key = content_vault(to, &id);
            for sealed in record_frames(&fs::read(&path)?) {
                key.open_sealed(CONTENT_AAD, sealed)
                    .map_err(|e| CoreError::Invalid(format!("copied content does not open: {e}")))?;
            }
        }
    }
    Ok(())
}

fn verify_file(path: &Path, key: &Vault, aad: &[u8]) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    key.open_sealed(aad, &fs::read(path)?)
        .map(|_| ())
        .map_err(|e| CoreError::Invalid(format!("copied {} does not open: {e}", display_name(path))))
}

fn display_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{append_sealed_records, read_sealed_records};
    use crate::avatars::{read_sealed, write_sealed};
    use dissent_identity::Identity;
    use std::collections::BTreeMap;

    fn scratch(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("dissent-reseal-{tag}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Every file under `dir` with its bytes, keyed by relative path.
    fn snapshot_of(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(root, &path, out);
                } else {
                    out.insert(path.strip_prefix(root).unwrap().to_path_buf(), fs::read(&path).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(dir, dir, &mut out);
        out
    }

    const FOUNDER_SEED: [u8; 32] = [11u8; 32];

    /// A profile sealed under `from`: identity, own and contact avatar files, and one community's content log.
    fn make_profile(root: &Path, from: &Vault, community: &CommunityId) -> Identity {
        let identity = Identity::create(1);
        keystore::save(&identity, &root.join(IDENTITY_FILE), from).unwrap();
        let profile = root.join("profiles").join(identity.root_id().to_string());
        fs::create_dir_all(&profile).unwrap();
        let cert = identity.root.as_ref().unwrap().certify(identity.device_id(), 1);
        let mls = MlsClient::new(&identity.device, &cert).unwrap();
        fs::write(profile.join(MLS_FILE), mls.snapshot_sealed(from).unwrap()).unwrap();
        write_sealed(&profile.join(AVATAR_FILE), &own_vault(from), &Some(7u32)).unwrap();
        write_sealed(&profile.join(PEER_AVATARS_FILE), &peers_vault(from), &Some(9u32)).unwrap();
        write_sealed(&profile.join(BANNERS_FILE), &peers_vault(from), &Some(11u32)).unwrap();
        let community_dir = profile.join("communities").join(community.to_string());
        let content = community_dir.join(CONTENT_FILE);
        append_sealed_records(&content, &content_vault(from, community), &[("hello".to_string(), 1u32), ("again".to_string(), 2u32)]).unwrap();
        write_community_key(&community_dir.join(COMMUNITY_KEY_FILE), from, community, &FOUNDER_SEED).unwrap();
        identity
    }

    #[test]
    fn a_copy_opens_under_the_new_vault_and_the_source_is_unchanged() {
        let root = scratch("round");
        let dest = scratch("round-dest").join("DissentData");
        let from = Vault::ephemeral_for_tests();
        let to = Vault::ephemeral_for_tests();
        let community = CommunityId([3u8; 32]);
        let identity = make_profile(&root, &from, &community);
        fs::write(root.join(LOCK_FILE), b"").unwrap();
        let before = snapshot_of(&root);

        copy_profile(&root, &from, &dest, &to, |_| Ok(())).unwrap();

        // The source is byte for byte what it was.
        assert_eq!(snapshot_of(&root), before);
        assert!(keystore::load(&root.join(IDENTITY_FILE), &from).is_ok());
        assert!(keystore::load(&root.join(IDENTITY_FILE), &to).is_err());

        // The copy opens with the new vault, and the old one no longer opens it.
        let copied = keystore::load(&dest.join(IDENTITY_FILE), &to).unwrap();
        assert_eq!(copied.device_id(), identity.device_id());
        assert!(keystore::load(&dest.join(IDENTITY_FILE), &from).is_err());
        let profile = dest.join("profiles").join(identity.root_id().to_string());
        let cert = identity.root.as_ref().unwrap().certify(identity.device_id(), 1);
        let snapshot = fs::read(profile.join(MLS_FILE)).unwrap();
        assert!(MlsClient::restore_sealed(&identity.device, &cert, &to, &snapshot, &[]).is_ok());
        assert!(MlsClient::restore_sealed(&identity.device, &cert, &from, &snapshot, &[]).is_err());
        assert_eq!(read_sealed::<Option<u32>>(&profile.join(AVATAR_FILE), &own_vault(&to)).unwrap(), Some(Some(7)));
        assert_eq!(read_sealed::<Option<u32>>(&profile.join(PEER_AVATARS_FILE), &peers_vault(&to)).unwrap(), Some(Some(9)));
        assert_eq!(read_sealed::<Option<u32>>(&profile.join(BANNERS_FILE), &peers_vault(&to)).unwrap(), Some(Some(11)));
        let content = profile.join("communities").join(community.to_string()).join(CONTENT_FILE);
        let records: Vec<(String, u32)> = read_sealed_records(&content, &content_vault(&to, &community)).unwrap();
        assert_eq!(records, vec![("hello".to_string(), 1), ("again".to_string(), 2)]);
        let key_file = profile.join("communities").join(community.to_string()).join(COMMUNITY_KEY_FILE);
        assert_eq!(read_community_key(&key_file, &to, &community).unwrap(), Some(FOUNDER_SEED));
        assert!(read_community_key(&key_file, &from, &community).is_err());

        // The lock is not carried, and nothing is left in staging.
        assert!(!dest.join(LOCK_FILE).exists());
        assert!(!dest.with_file_name("DissentData.copying").exists());
    }

    #[test]
    fn a_wrong_source_vault_leaves_nothing_behind() {
        let root = scratch("wrong");
        let dest = scratch("wrong-dest").join("DissentData");
        let from = Vault::ephemeral_for_tests();
        make_profile(&root, &from, &CommunityId([4u8; 32]));
        let wrong = Vault::ephemeral_for_tests();

        assert!(copy_profile(&root, &wrong, &dest, &Vault::ephemeral_for_tests(), |_| Ok(())).is_err());
        assert!(!dest.exists());
        assert!(!dest.with_file_name("DissentData.copying").exists());
    }

    #[test]
    fn prepare_runs_before_the_copy_appears() {
        let root = scratch("prepare");
        let dest = scratch("prepare-dest").join("DissentData");
        let from = Vault::ephemeral_for_tests();
        make_profile(&root, &from, &CommunityId([5u8; 32]));
        copy_profile(&root, &from, &dest, &Vault::ephemeral_for_tests(), |staged| {
            fs::write(staged.join("vault.salt"), [1u8; 16]).map_err(Into::into)
        })
        .unwrap();
        assert!(dest.join("vault.salt").exists());
    }

    #[test]
    fn an_existing_destination_is_refused() {
        let root = scratch("exists");
        let dest = scratch("exists-dest");
        let from = Vault::ephemeral_for_tests();
        make_profile(&root, &from, &CommunityId([6u8; 32]));
        assert!(copy_profile(&root, &from, &dest, &Vault::ephemeral_for_tests(), |_| Ok(())).is_err());
    }
}
