//! The sealed fields of tracker events, and how they are sealed. Each is postcard under XChaCha20-Poly1305 with a key
//! derived from the files key of a generation. The associated data binds the community, the author and the kind, so
//! a sealed body cannot be moved to another server, author or event kind.

use chacha20poly1305::aead::{Aead, KeyInit, Payload as AeadPayload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use dissent_events::FilesSealed;
use dissent_identity::{CommunityId, RootId};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::error::{CoreError, Result};

const SEAL_DOMAIN: &str = "dissent/files-sealed/v1";
const AAD_DOMAIN: &[u8] = b"dissent/files-sealed-aad/v1";
const NONCE_LEN: usize = 24;

/// Which tracker event a sealed body belongs to. Part of the associated data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BodyKind {
    Category = 1,
    Listing = 2,
    ListingUpdate = 3,
}

/// Coarse file types a category allows. Appended only: postcard writes the position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum FileType {
    Installer,
    Archive,
    Document,
    Image,
    Audio,
    Video,
    Other,
}

impl FileType {
    pub const ALL: [FileType; 7] =
        [Self::Installer, Self::Archive, Self::Document, Self::Image, Self::Audio, Self::Video, Self::Other];

    pub fn name(self) -> &'static str {
        match self {
            Self::Installer => "Installer",
            Self::Archive => "Archive",
            Self::Document => "Document",
            Self::Image => "Image",
            Self::Audio => "Audio",
            Self::Video => "Video",
            Self::Other => "Other",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.name().eq_ignore_ascii_case(name))
    }

    /// The type of a file, by its extension.
    pub fn of_name(name: &str) -> Self {
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
        match ext.as_str() {
            "msi" | "exe" | "dmg" | "pkg" | "appimage" | "deb" | "rpm" | "apk" | "msix" | "flatpak" | "snap" => {
                Self::Installer
            }
            "zip" | "7z" | "rar" | "tar" | "gz" | "tgz" | "zst" | "xz" | "bz2" | "lz4" => Self::Archive,
            "pdf" | "doc" | "docx" | "odt" | "txt" | "md" | "rtf" | "epub" | "xls" | "xlsx" | "ods" | "ppt" | "pptx"
            | "odp" | "csv" | "html" | "json" => Self::Document,
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "tif" | "tiff" | "heic" | "avif" | "ico" => Self::Image,
            "mp3" | "flac" | "wav" | "ogg" | "opus" | "m4a" | "aac" | "aiff" => Self::Audio,
            "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" | "wmv" => Self::Video,
            _ => Self::Other,
        }
    }
}

/// A category's own rules for downloading. Each app keeps them for its own member.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadRules {
    /// Ask for a ratio of at least this many thousandths, once `ratio_after_bytes` have been downloaded.
    pub min_ratio_milli: Option<u32>,
    pub ratio_after_bytes: u64,
    /// Ask people to seed for this many hours after they download.
    pub seed_hours: Option<u32>,
}

/// The sealed fields of a `FileCategorySet`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CategoryBody {
    pub name: String,
    /// An icon key the UI knows, or an emoji.
    pub icon: String,
    /// Allowed types. Empty allows every type.
    pub types: Vec<FileType>,
    pub default_tags: Vec<String>,
    pub rules: DownloadRules,
}

/// The sealed fields of a `FileListing`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListingBody {
    pub name: String,
    pub mime: String,
    pub kind: FileType,
    pub size: u64,
    pub fingerprint: [u8; 32],
    /// The files-key generation the content key comes from. A file listed again after a rotation keeps its first
    /// listing's generation, so its blob stays the same.
    pub content_generation: u64,
    pub blob: [u8; 32],
    pub cipher_size: u64,
    pub tags: Vec<String>,
    pub description: String,
}

/// The sealed fields of a `FileListingUpdate`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListingUpdateBody {
    pub tags: Vec<String>,
    pub description: String,
}

/// Longest tag and description, in characters, and most tags.
pub const MAX_TAG_CHARS: usize = 32;
pub const MAX_TAGS: usize = 16;
pub const MAX_DESCRIPTION_CHARS: usize = 2000;
/// Longest category name, in characters.
pub const MAX_CATEGORY_NAME_CHARS: usize = 48;

/// Tags as listings carry them: lowercased, without `#`, trimmed, unique and non-empty, within the limits.
pub fn clean_tags(tags: &[String]) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for tag in tags {
        let t = tag.trim().trim_start_matches('#').trim().to_lowercase();
        if t.is_empty() || out.contains(&t) {
            continue;
        }
        if t.chars().count() > MAX_TAG_CHARS || t.chars().any(char::is_whitespace) {
            return Err(CoreError::Invalid(format!("tag #{t} must be one word of at most {MAX_TAG_CHARS} characters")));
        }
        out.push(t);
    }
    if out.len() > MAX_TAGS {
        return Err(CoreError::Invalid(format!("at most {MAX_TAGS} tags")));
    }
    Ok(out)
}

/// A description as listings carry it: trimmed, within the limit.
pub fn clean_description(text: &str) -> Result<String> {
    let t = text.trim();
    if t.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(CoreError::Invalid(format!("a description is at most {MAX_DESCRIPTION_CHARS} characters")));
    }
    Ok(t.to_string())
}

fn aad(community: &CommunityId, author: &RootId, kind: BodyKind) -> Vec<u8> {
    [AAD_DOMAIN, community.as_bytes(), author.0.as_slice(), &[kind as u8]].concat()
}

fn cipher(files_key: &[u8; 32]) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new(Key::from_slice(&blake3::derive_key(SEAL_DOMAIN, files_key)))
}

/// Seals `body` under the files key of `generation`.
pub(crate) fn seal<T: Serialize>(
    files_key: &[u8; 32],
    generation: u64,
    community: &CommunityId,
    author: &RootId,
    kind: BodyKind,
    body: &T,
) -> Result<FilesSealed> {
    let plain = postcard::to_allocvec(body).map_err(|e| CoreError::Encoding(e.to_string()))?;
    let nonce: [u8; NONCE_LEN] = rand::random();
    let aad = aad(community, author, kind);
    let sealed = cipher(files_key)
        .encrypt(XNonce::from_slice(&nonce), AeadPayload { msg: &plain, aad: &aad })
        .map_err(|_| CoreError::Encoding("could not seal".into()))?;
    Ok(FilesSealed { generation, ciphertext: [nonce.as_slice(), &sealed].concat() })
}

/// Opens a sealed body with the files key of its generation. `None` when it does not open or decode.
pub(crate) fn open<T: DeserializeOwned>(
    files_key: &[u8; 32],
    community: &CommunityId,
    author: &RootId,
    kind: BodyKind,
    sealed: &FilesSealed,
) -> Option<T> {
    if sealed.ciphertext.len() < NONCE_LEN {
        return None;
    }
    let (nonce, ciphertext) = sealed.ciphertext.split_at(NONCE_LEN);
    let aad = aad(community, author, kind);
    let plain = cipher(files_key).decrypt(XNonce::from_slice(nonce), AeadPayload { msg: ciphertext, aad: &aad }).ok()?;
    postcard::from_bytes(&plain).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> CategoryBody {
        CategoryBody {
            name: "Releases".into(),
            icon: "package".into(),
            types: vec![FileType::Installer],
            default_tags: vec!["stable".into()],
            rules: DownloadRules::default(),
        }
    }

    #[test]
    fn a_body_opens_only_with_its_key_server_author_and_kind() {
        let (community, author, other) = (CommunityId([1; 32]), RootId([2; 32]), RootId([3; 32]));
        let sealed = seal(&[9; 32], 7, &community, &author, BodyKind::Category, &body()).unwrap();
        assert_eq!(sealed.generation, 7);
        assert_eq!(open::<CategoryBody>(&[9; 32], &community, &author, BodyKind::Category, &sealed), Some(body()));
        assert!(open::<CategoryBody>(&[8; 32], &community, &author, BodyKind::Category, &sealed).is_none());
        assert!(open::<CategoryBody>(&[9; 32], &CommunityId([4; 32]), &author, BodyKind::Category, &sealed).is_none());
        assert!(open::<CategoryBody>(&[9; 32], &community, &other, BodyKind::Category, &sealed).is_none());
        assert!(open::<CategoryBody>(&[9; 32], &community, &author, BodyKind::Listing, &sealed).is_none());
    }

    #[test]
    fn types_come_from_extensions() {
        assert_eq!(FileType::of_name("dissent-0.4.1-windows-x64.msi"), FileType::Installer);
        assert_eq!(FileType::of_name("x.AppImage"), FileType::Installer);
        assert_eq!(FileType::of_name("nightly.tar.zst"), FileType::Archive);
        assert_eq!(FileType::of_name("RELEASE-NOTES-draft.txt"), FileType::Document);
        assert_eq!(FileType::of_name("trailer.mp4"), FileType::Video);
        assert_eq!(FileType::of_name("noext"), FileType::Other);
        assert_eq!(FileType::parse("installer"), Some(FileType::Installer));
    }

    #[test]
    fn tags_are_cleaned() {
        let tags = clean_tags(&["#Stable".into(), "stable".into(), " windows ".into(), String::new()]).unwrap();
        assert_eq!(tags, vec!["stable", "windows"]);
        assert!(clean_tags(&["two words".into()]).is_err());
        assert!(clean_tags(&["x".repeat(33)]).is_err());
    }
}
