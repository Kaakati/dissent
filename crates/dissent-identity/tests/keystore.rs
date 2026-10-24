use dissent_identity::{keystore, DeviceCert, Identity};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

/// Mirror of the version 1 on-disk layout, to write legacy files.
#[derive(Serialize)]
struct V1 {
    version: u8,
    root_seed: Option<[u8; 32]>,
    device_seed: [u8; 32],
    cert: DeviceCert,
}

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

#[test]
fn reads_version_1_and_writes_version_2() {
    let path = temp_file();
    let id = Identity::create(3);
    let v1 = V1 {
        version: 1,
        root_seed: id.root.as_ref().map(|r| r.0.seed()),
        device_seed: id.device.0.seed(),
        cert: id.cert.clone(),
    };
    fs::write(&path, postcard::to_allocvec(&v1).unwrap()).unwrap();

    let loaded = keystore::load(&path).unwrap();
    assert_eq!(loaded.device_id(), id.device_id());
    assert_eq!(loaded.device_name, "");

    let mut named = loaded;
    named.device_name = "Laptop".into();
    keystore::save(&named, &path).unwrap();
    assert_eq!(fs::read(&path).unwrap()[0], 2);
    assert_eq!(keystore::load(&path).unwrap().device_name, "Laptop");
    cleanup(&path);
}

#[test]
fn rejects_unknown_version() {
    let path = temp_file();
    fs::write(&path, [9u8, 0, 0]).unwrap();
    assert!(keystore::load(&path).is_err());
    cleanup(&path);
}
