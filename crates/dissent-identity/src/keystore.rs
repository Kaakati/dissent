//! On-disk persistence of Identity.
//!
//! The first byte of a keystore file says how to read it:
//!
//! * `1` or `2`: legacy plaintext postcard. [`load`] reads it and rewrites it as version 3.
//! * `3`: sealed. `3 || Vault::seal(AAD, postcard(StoredIdentity))`. Only the vault can open it.

use crate::{DeviceCert, DeviceKey, Identity, IdentityError, Result, RootKey, Vault};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use zeroize::Zeroize;

const VERSION: u8 = 3;
/// Binds sealed keystore bytes to this file purpose, so they can't be swapped with another file.
const AAD: &[u8] = b"dissent/identity.bin";

/// Version 1 layout, still readable. Has no device name.
#[derive(Serialize, Deserialize)]
struct StoredV1 {
    version: u8,
    root_seed: Option<[u8; 32]>,
    device_seed: [u8; 32],
    cert: DeviceCert,
}

/// Versions 2 and 3 share this layout. Version 3 is only the sealed form of it.
#[derive(Serialize, Deserialize)]
struct StoredIdentity {
    version: u8,
    root_seed: Option<[u8; 32]>,
    device_seed: [u8; 32],
    cert: DeviceCert,
    device_name: String,
}

/// Decode a postcard body. Postcard puts the version byte first, so peek at it.
fn decode(bytes: &[u8]) -> Result<StoredIdentity> {
    let de = |e: postcard::Error| IdentityError::Serde(e.to_string());
    match bytes.first() {
        Some(1) => {
            let v1: StoredV1 = postcard::from_bytes(bytes).map_err(de)?;
            Ok(StoredIdentity {
                version: VERSION,
                root_seed: v1.root_seed,
                device_seed: v1.device_seed,
                cert: v1.cert,
                device_name: String::new(),
            })
        }
        Some(2 | 3) => postcard::from_bytes(bytes).map_err(de),
        other => Err(IdentityError::Serde(format!("unsupported keystore version {other:?}"))),
    }
}

/// Check the certificate chain and rebuild the identity.
fn identity_from(stored: StoredIdentity) -> Result<Identity> {
    let device = DeviceKey::from_seed(stored.device_seed);
    let root = stored.root_seed.map(RootKey::from_seed);
    stored.cert.verify()?;
    if stored.cert.device != device.id() {
        return Err(IdentityError::BadCert);
    }
    if root.as_ref().is_some_and(|r| r.id() != stored.cert.root) {
        return Err(IdentityError::BadCert);
    }
    Ok(Identity { root, device, cert: stored.cert, device_name: stored.device_name })
}

/// Seal the identity as a version 3 keystore file.
fn seal_identity(id: &Identity, vault: &Vault) -> Result<Vec<u8>> {
    let stored = StoredIdentity {
        version: VERSION,
        root_seed: id.root.as_ref().map(|r| r.0.seed()),
        device_seed: id.device.0.seed(),
        cert: id.cert.clone(),
        device_name: id.device_name.clone(),
    };
    let mut plain = postcard::to_allocvec(&stored).map_err(|e| IdentityError::Serde(e.to_string()))?;
    let mut out = vec![VERSION];
    out.extend(vault.seal(AAD, &plain));
    plain.zeroize();
    Ok(out)
}

/// Write the identity sealed under `vault`. Atomic: temp file, then rename. Unix mode is 0600.
pub fn save(id: &Identity, path: &Path, vault: &Vault) -> Result<()> {
    let bytes = seal_identity(id, vault)?;
    write_atomic(path, &bytes)
}

fn tmp_path(path: &Path) -> PathBuf {
    path.with_extension("tmp")
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = tmp_path(path);
    write_private(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    // Remove any leftover temp file so the new mode applies on creation.
    let _ = fs::remove_file(path);
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}

/// Read and verify an identity written by [`save`].
///
/// A legacy plaintext file (version 1 or 2) is verified, then rewritten as sealed version 3.
/// The rewrite writes the sealed copy to a temp file, overwrites the plaintext with zeros,
/// truncates it, and renames the sealed copy over the path.
///
/// Overwriting is best effort. SSD wear-levelling and copy-on-write filesystems can keep old
/// blocks, so zeroing is not a guarantee. Sealing is the real protection.
pub fn load(path: &Path, vault: &Vault) -> Result<Identity> {
    let bytes = fs::read(path)?;
    match bytes.first() {
        Some(&VERSION) => {
            let mut plain = vault.open_sealed(AAD, &bytes[1..])?;
            let stored = decode(&plain);
            plain.zeroize();
            identity_from(stored?)
        }
        Some(1 | 2) => {
            let id = identity_from(decode(&bytes)?)?;
            upgrade_plaintext(path, &id, bytes.len(), vault)?;
            Ok(id)
        }
        other => Err(IdentityError::Serde(format!("unsupported keystore version {other:?}"))),
    }
}

fn upgrade_plaintext(path: &Path, id: &Identity, old_len: usize, vault: &Vault) -> Result<()> {
    let sealed = seal_identity(id, vault)?;
    write_private(&tmp_path(path), &sealed)?;
    // Best effort. The sealed copy is already on disk, so a failed scrub only leaves old bytes.
    let _ = scrub_file(path, old_len);
    fs::rename(tmp_path(path), path)?;
    Ok(())
}

/// Overwrite the file with zeros, then truncate it to empty.
fn scrub_file(path: &Path, len: usize) -> std::io::Result<()> {
    let mut f = OpenOptions::new().write(true).open(path)?;
    f.write_all(&vec![0u8; len])?;
    f.sync_all()?;
    f.set_len(0)?;
    f.sync_all()
}

/// Load the identity at `path`, or create and save a fresh one if none exists.
pub fn load_or_create(path: &Path, now_ms: u64, vault: &Vault) -> Result<Identity> {
    match load(path, vault) {
        Err(IdentityError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            let id = Identity::create(now_ms);
            save(&id, path, vault)?;
            Ok(id)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_device_not_certified() {
        let dir = std::env::temp_dir().join(format!("dissent-ks-{:016x}", rand::random::<u64>()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("identity.bin");
        let id = Identity::create(1);
        // Legacy plaintext (version 2), so the load path reaches the certificate check.
        let stored = StoredIdentity {
            version: 2,
            root_seed: None,
            device_seed: DeviceKey::generate().0.seed(),
            cert: id.cert.clone(),
            device_name: String::new(),
        };
        fs::write(&path, postcard::to_allocvec(&stored).unwrap()).unwrap();
        assert!(matches!(load(&path, &Vault::ephemeral_for_tests()), Err(IdentityError::BadCert)));
        let _ = fs::remove_dir_all(&dir);
    }
}
