//! On-disk layout. `identity.bin` sits at the top of the data dir. Engine data for each identity
//! lives in `profiles/<root_id>/`, so importing another identity never shows the previous one's data.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use dissent_identity::Identity;
use tracing::info;

/// Engine entries written before per-identity folders existed. They belong to whichever identity was active then.
const LEGACY_ENTRIES: &[&str] = &["profile.bin", "contacts.bin", "communities", "dms"];

/// The engine data folder for this identity.
pub fn profile_dir(data_dir: &Path, identity: &Identity) -> PathBuf {
    data_dir.join("profiles").join(identity.root_id().to_string())
}

/// Moves engine data from before per-identity folders into `profile`. Runs once: it does nothing
/// when `profile` already exists or when no legacy entries are present.
pub fn migrate_legacy(data_dir: &Path, profile: &Path) -> io::Result<()> {
    if profile.exists() {
        return Ok(());
    }
    let found: Vec<&str> = LEGACY_ENTRIES
        .iter()
        .copied()
        .filter(|name| data_dir.join(name).exists())
        .collect();
    if found.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(profile)?;
    for name in found {
        fs::rename(data_dir.join(name), profile.join(name))?;
        info!(entry = name, "moved legacy engine data into the identity profile");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dissent-storage-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn moves_legacy_entries_once_and_keeps_identity() {
        let data = scratch("move");
        fs::write(data.join("identity.bin"), b"id").unwrap();
        fs::write(data.join("profile.bin"), b"p").unwrap();
        fs::create_dir_all(data.join("communities/abc")).unwrap();
        let profile = data.join("profiles/root1");

        migrate_legacy(&data, &profile).unwrap();
        assert!(profile.join("profile.bin").exists());
        assert!(profile.join("communities/abc").exists());
        assert!(!data.join("profile.bin").exists());
        assert!(data.join("identity.bin").exists());

        // A second identity must not receive the first one's data.
        let other = data.join("profiles/root2");
        migrate_legacy(&data, &other).unwrap();
        assert!(!other.exists());
        let _ = fs::remove_dir_all(&data);
    }
}
