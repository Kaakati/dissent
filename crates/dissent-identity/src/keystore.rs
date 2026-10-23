// chisle: plaintext keystore; wrap with OS keychain / passphrase before 1.0
//! On-disk persistence of Identity.

use crate::{DeviceCert, DeviceKey, Identity, IdentityError, Result, RootKey};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

const VERSION: u8 = 1;

#[derive(Serialize, Deserialize)]
struct StoredIdentity {
    version: u8,
    root_seed: Option<[u8; 32]>,
    device_seed: [u8; 32],
    cert: DeviceCert,
}

/// Write the identity atomically (temp file, then rename). Unix mode is 0600.
pub fn save(id: &Identity, path: &Path) -> Result<()> {
    let stored = StoredIdentity {
        version: VERSION,
        root_seed: id.root.as_ref().map(|r| r.0.seed()),
        device_seed: id.device.0.seed(),
        cert: id.cert.clone(),
    };
    let bytes = postcard::to_allocvec(&stored).map_err(|e| IdentityError::Serde(e.to_string()))?;
    let tmp = path.with_extension("tmp");
    write_private(&tmp, &bytes)?;
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
pub fn load(path: &Path) -> Result<Identity> {
    let bytes = fs::read(path)?;
    let stored: StoredIdentity =
        postcard::from_bytes(&bytes).map_err(|e| IdentityError::Serde(e.to_string()))?;
    if stored.version != VERSION {
        return Err(IdentityError::Serde(format!("unsupported keystore version {}", stored.version)));
    }
    let device = DeviceKey::from_seed(stored.device_seed);
    let root = stored.root_seed.map(RootKey::from_seed);
    stored.cert.verify()?;
    if stored.cert.device != device.id() {
        return Err(IdentityError::BadCert);
    }
    if root.as_ref().is_some_and(|r| r.id() != stored.cert.root) {
        return Err(IdentityError::BadCert);
    }
    Ok(Identity { root, device, cert: stored.cert })
}

/// Load the identity at `path`, or create and save a fresh one if none exists.
pub fn load_or_create(path: &Path, now_ms: u64) -> Result<Identity> {
    match load(path) {
        Err(IdentityError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            let id = Identity::create(now_ms);
            save(&id, path)?;
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
        let stored = StoredIdentity {
            version: VERSION,
            root_seed: None,
            device_seed: DeviceKey::generate().0.seed(),
            cert: id.cert.clone(),
        };
        fs::write(&path, postcard::to_allocvec(&stored).unwrap()).unwrap();
        assert!(matches!(load(&path), Err(IdentityError::BadCert)));
        let _ = fs::remove_dir_all(&dir);
    }
}
