use dissent_identity::{keystore, Identity};
use std::fs;
use std::path::{Path, PathBuf};

/// A fresh random directory under the OS temp dir, with `identity.bin` inside.
fn temp_file() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dissent-ks-{:016x}", rand::random::<u64>()));
    fs::create_dir_all(&dir).unwrap();
    dir.join("identity.bin")
}

fn cleanup(path: &Path) {
    let _ = fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn save_load_roundtrip() {
    let path = temp_file();
    let id = Identity::create(42);
    keystore::save(&id, &path).unwrap();
    let back = keystore::load(&path).unwrap();
    assert_eq!(back.root_id(), id.root_id());
    assert_eq!(back.device_id(), id.device_id());
    assert_eq!(back.cert, id.cert);
    assert!(back.root.is_some());
    assert!(!path.with_extension("tmp").exists());
    cleanup(&path);
}

#[test]
fn roundtrip_without_root_key() {
    let path = temp_file();
    let mut id = Identity::create(7);
    id.root = None;
    keystore::save(&id, &path).unwrap();
    let back = keystore::load(&path).unwrap();
    assert!(back.root.is_none());
    assert_eq!(back.device_id(), id.device_id());
    cleanup(&path);
}

#[test]
fn load_or_create_creates_once_then_reuses() {
    let path = temp_file();
    let first = keystore::load_or_create(&path, 1).unwrap();
    let second = keystore::load_or_create(&path, 999).unwrap();
    assert_eq!(first.device_id(), second.device_id());
    assert_eq!(second.cert.issued_at_ms, 1);
    cleanup(&path);
}
