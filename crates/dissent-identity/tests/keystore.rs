use dissent_identity::{keystore, DeviceCert, Identity, Vault};
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

/// Mirror of the version 2 on-disk layout (plaintext, with device name).
#[derive(Serialize)]
struct V2 {
    version: u8,
    root_seed: Option<[u8; 32]>,
    device_seed: [u8; 32],
    cert: DeviceCert,
    device_name: String,
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

fn contains(haystack: &[u8], needle: &[u8; 32]) -> bool {
    haystack.windows(32).any(|w| w == needle)
}

#[test]
fn save_load_roundtrip() {
    let path = temp_file();
    let vault = Vault::ephemeral_for_tests();
    let id = Identity::create(42);
    keystore::save(&id, &path, &vault).unwrap();
    let back = keystore::load(&path, &vault).unwrap();
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
    let vault = Vault::ephemeral_for_tests();
    let mut id = Identity::create(7);
    id.root = None;
    keystore::save(&id, &path, &vault).unwrap();
    let back = keystore::load(&path, &vault).unwrap();
    assert!(back.root.is_none());
    assert_eq!(back.device_id(), id.device_id());
    cleanup(&path);
}

#[test]
fn load_or_create_creates_once_then_reuses() {
    let path = temp_file();
    let vault = Vault::ephemeral_for_tests();
    let first = keystore::load_or_create(&path, 1, &vault).unwrap();
    let second = keystore::load_or_create(&path, 999, &vault).unwrap();
    assert_eq!(first.device_id(), second.device_id());
    assert_eq!(second.cert.issued_at_ms, 1);
    cleanup(&path);
}

#[test]
fn sealed_file_hides_seeds() {
    let path = temp_file();
    let vault = Vault::ephemeral_for_tests();
    let id = Identity::create(5);
    keystore::save(&id, &path, &vault).unwrap();
    let raw = fs::read(&path).unwrap();
    assert_eq!(raw[0], 3);
    assert!(!contains(&raw, &id.root.as_ref().unwrap().0.seed()));
    assert!(!contains(&raw, &id.device.0.seed()));
    cleanup(&path);
}

#[test]
fn reads_version_1_and_upgrades_to_version_3() {
    let path = temp_file();
    let vault = Vault::ephemeral_for_tests();
    let id = Identity::create(3);
    let v1 = V1 {
        version: 1,
        root_seed: id.root.as_ref().map(|r| r.0.seed()),
        device_seed: id.device.0.seed(),
        cert: id.cert.clone(),
    };
    fs::write(&path, postcard::to_allocvec(&v1).unwrap()).unwrap();

    let loaded = keystore::load(&path, &vault).unwrap();
    assert_eq!(loaded.device_id(), id.device_id());
    assert_eq!(loaded.device_name, "");
    assert_eq!(fs::read(&path).unwrap()[0], 3);

    let mut named = loaded;
    named.device_name = "Laptop".into();
    keystore::save(&named, &path, &vault).unwrap();
    assert_eq!(fs::read(&path).unwrap()[0], 3);
    assert_eq!(keystore::load(&path, &vault).unwrap().device_name, "Laptop");
    cleanup(&path);
}

#[test]
fn v2_plaintext_upgrades_to_sealed_and_removes_seeds() {
    let path = temp_file();
    let vault = Vault::ephemeral_for_tests();
    let id = Identity::create(11);
    let root_seed = id.root.as_ref().unwrap().0.seed();
    let device_seed = id.device.0.seed();
    let v2 = V2 {
        version: 2,
        root_seed: Some(root_seed),
        device_seed,
        cert: id.cert.clone(),
        device_name: "Desk".into(),
    };
    let plain = postcard::to_allocvec(&v2).unwrap();
    assert!(contains(&plain, &root_seed), "fixture must contain the seed");
    fs::write(&path, &plain).unwrap();

    let loaded = keystore::load(&path, &vault).unwrap();
    assert_eq!(loaded.root_id(), id.root_id());
    assert_eq!(loaded.device_name, "Desk");

    let raw = fs::read(&path).unwrap();
    assert_eq!(raw[0], 3, "file is rewritten as version 3");
    assert!(!contains(&raw, &root_seed), "root seed must not appear on disk");
    assert!(!contains(&raw, &device_seed), "device seed must not appear on disk");
    assert!(!path.with_extension("tmp").exists());

    let again = keystore::load(&path, &vault).unwrap();
    assert_eq!(again.device_id(), id.device_id());
    assert_eq!(again.device_name, "Desk");
    cleanup(&path);
}

#[test]
fn wrong_vault_cannot_open() {
    let path = temp_file();
    let id = Identity::create(13);
    keystore::save(&id, &path, &Vault::ephemeral_for_tests()).unwrap();
    assert!(keystore::load(&path, &Vault::ephemeral_for_tests()).is_err());
    cleanup(&path);
}

#[test]
fn tampered_sealed_file_fails() {
    let path = temp_file();
    let vault = Vault::ephemeral_for_tests();
    let id = Identity::create(17);
    keystore::save(&id, &path, &vault).unwrap();
    let mut raw = fs::read(&path).unwrap();
    let last = raw.len() - 1;
    raw[last] ^= 0x01;
    fs::write(&path, &raw).unwrap();
    assert!(keystore::load(&path, &vault).is_err());
    cleanup(&path);
}

#[test]
fn rejects_unknown_version() {
    let path = temp_file();
    fs::write(&path, [9u8, 0, 0]).unwrap();
    assert!(keystore::load(&path, &Vault::ephemeral_for_tests()).is_err());
    cleanup(&path);
}
