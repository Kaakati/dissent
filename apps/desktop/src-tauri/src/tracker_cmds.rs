//! The server file tracker: the Files area and the Files section of Server Settings. See `dissent_core::tracker`.

use std::path::PathBuf;

use dissent_core::tracker::adding::PickedView;
use dissent_core::tracker::catalog::{AddItem, CategoryInput, FilesView};
use dissent_core::tracker::drops::DropPlan;
use dissent_core::tracker::folders::FolderScan;
use dissent_core::tracker::live::LiveSwarm;
use dissent_core::tracker::swarm::SwarmView;
use dissent_core::tracker::seeding::{parse_blob, parse_limit, SeedingSettings, SeedingView};
use dissent_core::{CommunityId, RootId, TransferView};
use dissent_events::{CategoryId, EventId, ReputationRules};
use tauri::State;

use crate::commands::{engine, err, parse, AppState};

#[tauri::command]
pub async fn files_view(state: State<'_, AppState>, community_id: String) -> Result<FilesView, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.files_view(community).map_err(err)
}

#[tauri::command]
pub async fn create_category(state: State<'_, AppState>, community_id: String, category: CategoryInput) -> Result<String, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let id = engine(&state).await?.create_category(community, category).await.map_err(err)?;
    Ok(id.to_string())
}

#[tauri::command]
pub async fn update_category(
    state: State<'_, AppState>,
    community_id: String,
    category_id: String,
    category: CategoryInput,
) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let id: CategoryId = parse("category id", &category_id)?;
    engine(&state).await?.update_category(community, id, category).await.map_err(err)
}

#[tauri::command]
pub async fn delete_category(state: State<'_, AppState>, community_id: String, category_id: String) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let id: CategoryId = parse("category id", &category_id)?;
    engine(&state).await?.delete_category(community, id).await.map_err(err)
}

/// Asks for a folder through the OS picker. `None` when it is cancelled. See the `test-folder-hook` feature.
pub(crate) async fn ask_folder(app: tauri::AppHandle) -> Result<Option<PathBuf>, String> {
    use tauri_plugin_dialog::DialogExt;
    #[cfg(feature = "test-folder-hook")]
    if let Some(file) = std::env::var_os("DISSENT_TEST_PICK_FOLDER_FILE") {
        let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
        return Ok(Some(PathBuf::from(text.trim())));
    }
    let picked = tauri::async_runtime::spawn_blocking(move || app.dialog().file().blocking_pick_folder())
        .await
        .map_err(|e| e.to_string())?;
    picked.map(|f| f.into_path().map_err(err)).transpose()
}

/// Opens a folder picker and links the chosen folder to the category on this computer. Returns the path, or `None`
/// when the picker was cancelled.
#[tauri::command]
pub async fn pick_category_folder(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    community_id: String,
    category_id: String,
) -> Result<Option<String>, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let id: CategoryId = parse("category id", &category_id)?;
    let Some(folder) = ask_folder(app).await? else {
        return Ok(None);
    };
    engine(&state).await?.set_category_folder(community, id, Some(folder.clone())).map_err(err)?;
    Ok(Some(folder.to_string_lossy().into_owned()))
}

#[tauri::command]
pub async fn unlink_category_folder(state: State<'_, AppState>, community_id: String, category_id: String) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let id: CategoryId = parse("category id", &category_id)?;
    engine(&state).await?.set_category_folder(community, id, None).map_err(err)
}

#[tauri::command]
pub async fn set_category_auto_import(
    state: State<'_, AppState>,
    community_id: String,
    category_id: String,
    on: bool,
) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let id: CategoryId = parse("category id", &category_id)?;
    engine(&state).await?.set_category_auto_import(community, id, on).map_err(err)
}

/// Opens a file picker and reports what was picked, without reading the files. See `start_file_check`.
#[tauri::command]
pub async fn pick_listing_files(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<Vec<PickedView>, String> {
    use tauri_plugin_dialog::DialogExt;
    let picked = tauri::async_runtime::spawn_blocking(move || app.dialog().file().blocking_pick_files())
        .await
        .map_err(|e| e.to_string())?;
    let paths: Vec<PathBuf> = picked.unwrap_or_default().into_iter().filter_map(|f| f.into_path().ok()).collect();
    let engine = engine(&state).await?;
    engine.allow_paths(paths.iter().cloned());
    Ok(engine.describe_paths(&paths))
}

/// The files in the category's linked folder this computer does not seed yet, without reading them.
#[tauri::command]
pub async fn scan_category_folder(state: State<'_, AppState>, community_id: String, category_id: String) -> Result<Vec<PickedView>, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let id: CategoryId = parse("category id", &category_id)?;
    let engine = engine(&state).await?;
    let found = engine.new_files_in_folder(community, id).await.map_err(err)?;
    Ok(engine.describe_paths(&found))
}

/// Fingerprints chosen files in the background, each reported as a `file_check` event. Returns the job.
#[tauri::command]
pub async fn start_file_check(state: State<'_, AppState>, community_id: String, paths: Vec<PathBuf>) -> Result<u64, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.start_file_check(community, paths).map_err(err)
}

#[tauri::command]
pub async fn cancel_file_check(state: State<'_, AppState>, job: u64) -> Result<(), String> {
    engine(&state).await?.cancel_file_check(job);
    Ok(())
}

/// Opens a folder picker for adding a whole folder. `None` when it is cancelled.
#[tauri::command]
pub async fn pick_listing_folder(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<Option<PathBuf>, String> {
    let folder = ask_folder(app).await?;
    if let Some(folder) = &folder {
        engine(&state).await?.allow_paths([folder.clone()]);
    }
    Ok(folder)
}

/// What a chosen or dropped folder holds, and what was left out and why.
#[tauri::command]
pub async fn scan_listing_folder(state: State<'_, AppState>, community_id: String, folder: PathBuf, filter: Option<String>) -> Result<FolderScan, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let filter: Option<CategoryId> = filter.map(|f| parse("category id", &f)).transpose()?;
    engine(&state).await?.scan_folder(community, folder, filter).await.map_err(err)
}

/// What a drop on the Files page will do with `paths`. A server with no category gets them here, when this person may make
/// them.
#[tauri::command]
pub async fn plan_file_drop(state: State<'_, AppState>, community_id: String, paths: Vec<PathBuf>, filter: Option<String>) -> Result<DropPlan, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let filter: Option<CategoryId> = filter.map(|f| parse("category id", &f)).transpose()?;
    engine(&state).await?.plan_file_drop(community, paths, filter).await.map_err(err)
}

/// `limit` is `forever`, `ratio:<thousandths>`, `hours:<n>` or `until:<ms>`.
#[tauri::command]
pub async fn list_files(
    state: State<'_, AppState>,
    community_id: String,
    category_id: String,
    items: Vec<AddItem>,
    limit: String,
    in_place: Option<bool>,
) -> Result<Vec<String>, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let id: CategoryId = parse("category id", &category_id)?;
    let limit = parse_limit(&limit).map_err(err)?;
    let engine = engine(&state).await?;
    if in_place.unwrap_or(false) {
        engine.list_files_in_place(community, id, items, limit).await.map_err(err)
    } else {
        engine.list_files(community, id, items, limit).await.map_err(err)
    }
}


#[tauri::command]
pub async fn update_listing(
    state: State<'_, AppState>,
    community_id: String,
    listing_id: String,
    tags: Vec<String>,
    description: String,
) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let id: EventId = parse("listing id", &listing_id)?;
    engine(&state).await?.update_listing(community, id, tags, description).await.map_err(err)
}

#[tauri::command]
pub async fn remove_listing(state: State<'_, AppState>, community_id: String, listing_id: String) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let id: EventId = parse("listing id", &listing_id)?;
    engine(&state).await?.remove_listing(community, id).await.map_err(err)
}

#[tauri::command]
pub async fn download_listing(state: State<'_, AppState>, community_id: String, listing_id: String) -> Result<TransferView, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let id: EventId = parse("listing id", &listing_id)?;
    engine(&state).await?.download_listing(community, id).await.map_err(err)
}

#[tauri::command]
pub async fn listing_swarm(state: State<'_, AppState>, community_id: String, listing_id: String) -> Result<SwarmView, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let id: EventId = parse("listing id", &listing_id)?;
    engine(&state).await?.listing_swarm(community, id).await.map_err(err)
}

#[tauri::command]
pub async fn listing_live(state: State<'_, AppState>, community_id: String, listing_id: String) -> Result<LiveSwarm, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let id: EventId = parse("listing id", &listing_id)?;
    engine(&state).await?.listing_live(community, id).await.map_err(err)
}

#[tauri::command]
pub async fn set_reputation_rules(state: State<'_, AppState>, community_id: String, rules: ReputationRules) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.set_reputation_rules(community, rules).await.map_err(err)
}

#[tauri::command]
pub async fn reset_reputation(state: State<'_, AppState>, community_id: String, user_id: String) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let user: RootId = parse("user id", &user_id)?;
    engine(&state).await?.reset_reputation(community, user).await.map_err(err)
}

fn blob_of(text: &str) -> Result<[u8; 32], String> {
    parse_blob(text).map_err(err)
}

#[tauri::command]
pub async fn seeding_view(state: State<'_, AppState>, community_id: String) -> Result<SeedingView, String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.seeding_view(community).map_err(err)
}

#[tauri::command]
pub async fn set_seeding_settings(state: State<'_, AppState>, settings: SeedingSettings) -> Result<(), String> {
    engine(&state).await?.set_seeding_settings(settings).map_err(err)
}

/// `limit` as for `list_files`.
#[tauri::command]
pub async fn set_seed_limit(state: State<'_, AppState>, community_id: String, blob: String, limit: String) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    let limit = parse_limit(&limit).map_err(err)?;
    engine(&state).await?.set_seed_limit(community, blob_of(&blob)?, limit).map_err(err)
}

#[tauri::command]
pub async fn pause_seed(state: State<'_, AppState>, community_id: String, blob: String, paused: bool) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.pause_seed(community, blob_of(&blob)?, paused).map_err(err)
}

#[tauri::command]
pub async fn stop_seed(state: State<'_, AppState>, community_id: String, blob: String) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.stop_seed(community, blob_of(&blob)?).map_err(err)
}

#[tauri::command]
pub async fn pause_all_seeds(state: State<'_, AppState>, community_id: String, paused: bool) -> Result<(), String> {
    let community: CommunityId = parse("community id", &community_id)?;
    engine(&state).await?.pause_all_seeds(community, paused).map_err(err)
}

/// The UI's local time offset (`-Date.getTimezoneOffset()`), for the alternative schedule.
#[tauri::command]
pub async fn set_utc_offset(state: State<'_, AppState>, minutes: i32) -> Result<(), String> {
    engine(&state).await?.set_utc_offset_min(minutes);
    Ok(())
}
