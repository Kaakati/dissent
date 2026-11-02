//! Dropping files on the Files page: where each one goes, from its type. See `docs/design/14-file-tracker.md`, "Adding files".

use std::path::{Path, PathBuf};

use dissent_events::{CategoryId, Perms};
use dissent_files::MAX_FILE_SIZE;
use dissent_identity::CommunityId;
use serde::Serialize;

use super::adding::PickedView;
use super::bodies::{open, BodyKind, CategoryBody, DownloadRules, FileType};
use super::catalog::{AddSpecView, CategoryInput};
use crate::engine::{now_ms, Engine};
use crate::error::Result;

/// Why a dropped file is not listed. Listed files have none.
#[derive(Clone, Debug, Serialize)]
pub struct DropItem {
    pub path: String,
    pub name: String,
    pub ext: String,
    pub kind: String,
    pub size: u64,
    pub category: Option<String>,
    pub category_name: Option<String>,
    /// "no_category", "too_large", "unreadable", "link" or "credentials".
    pub skip: Option<String>,
}

/// What a drop on the Files page will do.
#[derive(Clone, Debug, Serialize)]
pub struct DropPlan {
    pub items: Vec<DropItem>,
    /// Dropped folders. Each opens the folder dialog.
    pub folders: Vec<String>,
    /// Categories this drop made, for a server with none.
    pub created: Vec<String>,
    /// The server has no category and this person may not make one.
    pub no_categories: bool,
}

/// A category this person may add to, with what it accepts.
#[derive(Clone, Debug)]
pub(crate) struct CategoryChoice {
    pub id: CategoryId,
    pub name: String,
    pub types: Vec<FileType>,
}

impl CategoryChoice {
    fn accepts(&self, kind: FileType) -> bool {
        self.types.is_empty() || self.types.contains(&kind)
    }
}

/// Where a file of `kind` goes: the filter's category if it accepts it, then the narrowest category that names the type,
/// then one that takes any type. `choices` is in server order, so ties go to the first.
pub(crate) fn choose_category(kind: FileType, choices: &[CategoryChoice], filter: Option<CategoryId>) -> Option<&CategoryChoice> {
    if let Some(chosen) = filter.and_then(|f| choices.iter().find(|c| c.id == f && c.accepts(kind))) {
        return Some(chosen);
    }
    choices
        .iter()
        .filter(|c| c.types.contains(&kind))
        .min_by_key(|c| c.types.len())
        .or_else(|| choices.iter().find(|c| c.types.is_empty()))
}

/// What a category made for a server with none is called.
fn category_name_for(kind: FileType) -> &'static str {
    match kind {
        FileType::Installer => "Installers",
        FileType::Archive => "Archives",
        FileType::Document => "Documents",
        FileType::Image => "Images",
        FileType::Audio => "Audio",
        FileType::Video => "Videos",
        FileType::Other => "Other files",
    }
}

/// Whether a name looks like a secret someone did not mean to share.
pub(crate) fn looks_like_credentials(name: &str) -> bool {
    const NAMES: [&str; 14] = [
        ".env", "id_rsa", "id_dsa", "id_ecdsa", "id_ed25519", "credentials", "credentials.json", "secrets.json", ".npmrc",
        ".pypirc", ".netrc", ".pgpass", ".git-credentials", "wallet.dat",
    ];
    const EXTENSIONS: [&str; 7] = ["pem", "key", "pfx", "p12", "ppk", "kdbx", "keystore"];
    let lower = name.to_ascii_lowercase();
    NAMES.contains(&lower.as_str())
        || lower.starts_with(".env.")
        || lower.rsplit_once('.').is_some_and(|(_, ext)| EXTENSIONS.contains(&ext))
}

impl Engine {
    /// The categories this person may add to, in server order. One this device cannot open is left out.
    pub(crate) fn category_choices(&self, community: CommunityId) -> Vec<CategoryChoice> {
        let keys = self.with_tracker(community, |l| l.keys.clone());
        let w = self.read();
        let Some(c) = w.communities.get(&community) else { return Vec::new() };
        let st = c.state();
        let now = now_ms();
        let mut found: Vec<(dissent_events::Hlc, CategoryChoice)> = st
            .files
            .categories
            .iter()
            .filter(|(id, _)| st.may_see_files(&w.me, id) && st.may_add_files(&w.me, id, now))
            .filter_map(|(id, cat)| {
                let body: CategoryBody = open(keys.get(&cat.inner.generation)?, &community, &cat.author, BodyKind::Category, &cat.inner)?;
                Some((cat.created, CategoryChoice { id: *id, name: body.name, types: body.types }))
            })
            .collect();
        found.sort_by_key(|(created, _)| *created);
        found.into_iter().map(|(_, choice)| choice).collect()
    }

    /// What dropping `paths` on the Files page does: where each file goes and why any does not, and which are folders. A
    /// server with no category gets one for each type dropped, when this person may make them. `filter` is the category the
    /// page shows.
    pub async fn plan_file_drop(&self, community: CommunityId, paths: Vec<PathBuf>, filter: Option<CategoryId>) -> Result<DropPlan> {
        self.check_allowed(&paths)?;
        let views: Vec<PickedView> = self.describe_paths(&paths);
        let folders: Vec<String> = views.iter().filter(|v| v.is_dir).map(|v| v.path.clone()).collect();
        let files: Vec<&PickedView> = views.iter().filter(|v| !v.is_dir).collect();

        let me = self.inner.me;
        let (none, manage) = {
            let w = self.read();
            let c = w.communities.get(&community).ok_or_else(|| crate::engine::not_found(&community))?;
            let st = c.state();
            (st.files.categories.is_empty(), st.perms_of(&me).contains(Perms::MANAGE_FILES))
        };
        let mut created = Vec::new();
        if none && manage {
            let mut kinds: Vec<FileType> = Vec::new();
            for file in &files {
                let kind = FileType::of_name(&file.name);
                if !kinds.contains(&kind) && !looks_like_credentials(&file.name) && file.size <= MAX_FILE_SIZE {
                    kinds.push(kind);
                }
            }
            for kind in kinds {
                let name = category_name_for(kind).to_string();
                let input = CategoryInput {
                    name: name.clone(),
                    icon: "folder".into(),
                    types: vec![kind.name().to_string()],
                    default_tags: Vec::new(),
                    rules: DownloadRules::default(),
                    add: AddSpecView { min_rank: "member".into(), ..AddSpecView::default() },
                    visible: None,
                };
                self.create_category(community, input).await?;
                created.push(name);
            }
        }
        let choices = self.category_choices(community);
        let items = files
            .into_iter()
            .map(|file| {
                let path = Path::new(&file.path);
                let meta = std::fs::symlink_metadata(path).ok();
                let kind = FileType::of_name(&file.name);
                let choice = choose_category(kind, &choices, filter);
                let skip = match &meta {
                    None => Some("unreadable"),
                    Some(m) if m.file_type().is_symlink() => Some("link"),
                    Some(m) if m.len() > MAX_FILE_SIZE => Some("too_large"),
                    // A key file listed by accident goes to the whole server, and a drop asks nothing first.
                    Some(_) if looks_like_credentials(&file.name) => Some("credentials"),
                    Some(_) if choice.is_none() => Some("no_category"),
                    Some(_) => None,
                };
                DropItem {
                    path: file.path.clone(),
                    name: file.name.clone(),
                    ext: file.ext.clone(),
                    kind: file.kind.clone(),
                    size: file.size,
                    category: choice.filter(|_| skip.is_none()).map(|c| c.id.to_string()),
                    category_name: choice.filter(|_| skip.is_none()).map(|c| c.name.clone()),
                    skip: skip.map(String::from),
                }
            })
            .collect();
        Ok(DropPlan { items, folders, created, no_categories: none && !manage })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(n: u8, name: &str, types: &[FileType]) -> CategoryChoice {
        CategoryChoice { id: CategoryId([n; 32]), name: name.into(), types: types.to_vec() }
    }

    #[test]
    fn a_file_goes_to_the_narrowest_category_that_names_its_type() {
        let choices = [
            choice(1, "Anything", &[]),
            choice(2, "Media", &[FileType::Image, FileType::Video]),
            choice(3, "Pictures", &[FileType::Image]),
            choice(4, "Also pictures", &[FileType::Image]),
        ];
        assert_eq!(choose_category(FileType::Image, &choices, None).unwrap().name, "Pictures", "narrowest, first of a tie");
        assert_eq!(choose_category(FileType::Video, &choices, None).unwrap().name, "Media");
        assert_eq!(choose_category(FileType::Archive, &choices, None).unwrap().name, "Anything", "then any type");
    }

    #[test]
    fn the_page_filter_wins_only_when_it_accepts_the_file() {
        let choices = [choice(1, "Media", &[FileType::Image, FileType::Video]), choice(2, "Pictures", &[FileType::Image]), choice(3, "Anything", &[])];
        assert_eq!(choose_category(FileType::Image, &choices, Some(CategoryId([1; 32]))).unwrap().name, "Media");
        assert_eq!(choose_category(FileType::Image, &choices, Some(CategoryId([3; 32]))).unwrap().name, "Anything");
        assert_eq!(choose_category(FileType::Archive, &choices, Some(CategoryId([2; 32]))).unwrap().name, "Anything", "the filter refuses it");
        assert_eq!(choose_category(FileType::Archive, &choices[..2], Some(CategoryId([2; 32]))).map(|c| c.name.as_str()), None);
        assert!(choose_category(FileType::Image, &[], None).is_none());
    }

    #[test]
    fn likely_credentials_are_recognised() {
        for name in [".env", ".env.local", "id_rsa", "ID_ED25519", "server.pem", "my.KEY", "cert.pfx", ".git-credentials"] {
            assert!(looks_like_credentials(name), "{name}");
        }
        for name in ["photo.png", "environment.md", "keyboard.txt", "id_rsa.pub", "monkey", "notes.env.txt"] {
            assert!(!looks_like_credentials(name), "{name}");
        }
    }
}
