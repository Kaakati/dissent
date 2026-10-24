//! Registrations in memory, with an optional JSON snapshot on disk. Wake tokens and delete secrets
//! are stored only as hashes. Endpoints are stored as given, because the relay must send to them.

use crate::PushError;
use crate::token::{ct_eq, hash_secret};
use data_encoding::HEXLOWER;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Where a wake goes. The URL or token is the only thing the relay learns about the device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Endpoint {
    /// A UnifiedPush distributor endpoint.
    #[serde(rename = "unifiedpush")]
    UnifiedPush { url: String },
    /// An APNs device token, hex encoded.
    Apns { device_token: String },
}

#[derive(Clone, Debug)]
pub struct Registration {
    pub secret_hash: [u8; 32],
    pub endpoint: Endpoint,
}

/// Outcome of an authorised delete.
#[derive(Debug, PartialEq, Eq)]
pub enum Removal {
    Removed,
    NotFound,
    WrongSecret,
}

#[derive(Serialize, Deserialize)]
struct SnapshotEntry {
    token_hash: String,
    secret_hash: String,
    endpoint: Endpoint,
}

#[derive(Serialize, Deserialize)]
struct Snapshot {
    version: u32,
    registrations: Vec<SnapshotEntry>,
}

const SNAPSHOT_VERSION: u32 = 1;

pub struct Store {
    map: Mutex<HashMap<[u8; 32], Registration>>,
    path: Option<PathBuf>,
}

impl Store {
    /// An empty store. If `path` exists, its registrations are loaded. Changes are written back
    /// to `path` by [`Store::persist`].
    pub fn open(path: Option<PathBuf>) -> Result<Self, PushError> {
        let map = match path.as_deref().filter(|p| p.exists()) {
            Some(p) => load(p)?,
            None => HashMap::new(),
        };
        Ok(Self {
            map: Mutex::new(map),
            path,
        })
    }

    pub fn insert(&self, token_hash: [u8; 32], registration: Registration) {
        self.lock().insert(token_hash, registration);
    }

    pub fn endpoint(&self, token_hash: &[u8; 32]) -> Option<Endpoint> {
        self.lock().get(token_hash).map(|r| r.endpoint.clone())
    }

    /// Removes the registration if `secret` is the one it was issued with.
    pub fn remove_with_secret(&self, token_hash: &[u8; 32], secret: &[u8]) -> Removal {
        let mut map = self.lock();
        let Some(reg) = map.get(token_hash) else {
            return Removal::NotFound;
        };
        if !ct_eq(&reg.secret_hash, &hash_secret(secret)) {
            return Removal::WrongSecret;
        }
        map.remove(token_hash);
        Removal::Removed
    }

    /// Drops a registration without a secret. Used when the push provider says it is gone.
    pub fn remove(&self, token_hash: &[u8; 32]) {
        self.lock().remove(token_hash);
    }

    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Writes the snapshot, if a path is set. Writes to a temporary file, then renames it.
    pub fn persist(&self) -> Result<(), PushError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let snapshot = {
            let map = self.lock();
            Snapshot {
                version: SNAPSHOT_VERSION,
                registrations: map
                    .iter()
                    .map(|(token_hash, reg)| SnapshotEntry {
                        token_hash: HEXLOWER.encode(token_hash),
                        secret_hash: HEXLOWER.encode(&reg.secret_hash),
                        endpoint: reg.endpoint.clone(),
                    })
                    .collect(),
            }
        };
        let text = serde_json::to_string_pretty(&snapshot)
            .map_err(|e| PushError::Storage(e.to_string()))?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, text).map_err(|e| PushError::Storage(e.to_string()))?;
        std::fs::rename(&tmp, path).map_err(|e| PushError::Storage(e.to_string()))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<[u8; 32], Registration>> {
        self.map.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn load(path: &Path) -> Result<HashMap<[u8; 32], Registration>, PushError> {
    let text = std::fs::read_to_string(path).map_err(|e| PushError::Storage(e.to_string()))?;
    let snapshot: Snapshot =
        serde_json::from_str(&text).map_err(|e| PushError::Storage(e.to_string()))?;
    if snapshot.version != SNAPSHOT_VERSION {
        return Err(PushError::Storage(format!(
            "unsupported snapshot version {}",
            snapshot.version
        )));
    }
    let mut map = HashMap::new();
    for entry in snapshot.registrations {
        let token_hash = decode_hash(&entry.token_hash)?;
        let secret_hash = decode_hash(&entry.secret_hash)?;
        map.insert(
            token_hash,
            Registration {
                secret_hash,
                endpoint: entry.endpoint,
            },
        );
    }
    Ok(map)
}

fn decode_hash(text: &str) -> Result<[u8; 32], PushError> {
    let raw = HEXLOWER
        .decode(text.as_bytes())
        .map_err(|e| PushError::Storage(format!("bad hash in snapshot: {e}")))?;
    <[u8; 32]>::try_from(raw.as_slice())
        .map_err(|_| PushError::Storage("hash in snapshot has the wrong length".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::{decode_token, encode, hash_token, random_bytes};

    fn temp_path(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "dissent-push-relay-{name}-{}-{nanos}.json",
            std::process::id()
        ))
    }

    #[test]
    fn snapshot_holds_no_usable_tokens_or_secrets() {
        let path = temp_path("at-rest");
        let store = Store::open(Some(path.clone())).unwrap();
        let token: [u8; 16] = random_bytes().unwrap();
        let secret: [u8; 32] = random_bytes().unwrap();
        store.insert(
            hash_token(&token),
            Registration {
                secret_hash: hash_secret(&secret),
                endpoint: Endpoint::UnifiedPush {
                    url: "https://push.example.org/abc".into(),
                },
            },
        );
        store.persist().unwrap();

        let dump = std::fs::read_to_string(&path).unwrap();
        assert!(!dump.contains(&encode(&token)), "wake token written in clear");
        assert!(!dump.contains(&encode(&secret)), "delete secret written in clear");
        assert!(!dump.contains(&HEXLOWER.encode(&token)), "token bytes in clear");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn reloaded_snapshot_still_wakes_and_authorises_deletes() {
        let path = temp_path("reload");
        let token: [u8; 16] = random_bytes().unwrap();
        let secret: [u8; 32] = random_bytes().unwrap();
        {
            let store = Store::open(Some(path.clone())).unwrap();
            store.insert(
                hash_token(&token),
                Registration {
                    secret_hash: hash_secret(&secret),
                    endpoint: Endpoint::Apns {
                        device_token: "ab".repeat(32),
                    },
                },
            );
            store.persist().unwrap();
        }
        let store = Store::open(Some(path.clone())).unwrap();
        assert_eq!(store.len(), 1);
        let text = encode(&token);
        let hash = hash_token(&decode_token(&text).unwrap());
        assert!(matches!(store.endpoint(&hash), Some(Endpoint::Apns { .. })));
        assert_eq!(store.remove_with_secret(&hash, &secret), Removal::Removed);
        assert!(store.is_empty());
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn delete_needs_the_matching_secret() {
        let store = Store::open(None).unwrap();
        let token = [3u8; 16];
        let hash = hash_token(&token);
        assert_eq!(store.remove_with_secret(&hash, b"x"), Removal::NotFound);
        store.insert(
            hash,
            Registration {
                secret_hash: hash_secret(b"right"),
                endpoint: Endpoint::UnifiedPush {
                    url: "https://a.example.org/".into(),
                },
            },
        );
        assert_eq!(store.remove_with_secret(&hash, b"wrong"), Removal::WrongSecret);
        assert!(store.endpoint(&hash).is_some());
        assert_eq!(store.remove_with_secret(&hash, b"right"), Removal::Removed);
        assert!(store.endpoint(&hash).is_none());
    }
}
