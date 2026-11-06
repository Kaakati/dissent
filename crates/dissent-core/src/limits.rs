//! File size limits: the largest file this person shares and downloads, kept on this device, and the largest a server
//! lets its members share (`Payload::ServerFileSizeLimit`). Each defaults to 10 GiB and may be set anywhere from 1 MiB to
//! the file format's ceiling of 1 TiB. Sending keeps to the smaller of this device's upload limit and the server's; a file
//! over this device's download limit, or over the server's, is never fetched on arrival.

use dissent_events::{DEFAULT_FILE_SIZE_LIMIT, MAX_FILE_SIZE_LIMIT, MIN_FILE_SIZE_LIMIT};
use serde::{Deserialize, Serialize};

use crate::engine::Engine;
use crate::error::{CoreError, Result};

/// The prefix of the error a download over this device's limit fails with, so the app can offer to go ahead anyway.
pub const OVER_DOWNLOAD_LIMIT: &str = "over_download_limit";

/// This device's limits, in plaintext bytes. Kept in `size_limits.bin`, sealed like every store file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SizeLimits {
    pub max_upload: u64,
    pub max_download: u64,
}

impl Default for SizeLimits {
    fn default() -> Self {
        Self { max_upload: DEFAULT_FILE_SIZE_LIMIT, max_download: DEFAULT_FILE_SIZE_LIMIT }
    }
}

/// Whether `bytes` is a limit anyone may set: 1 MiB to 1 TiB.
pub fn valid_limit(bytes: u64) -> bool {
    (MIN_FILE_SIZE_LIMIT..=MAX_FILE_SIZE_LIMIT).contains(&bytes)
}

/// The largest file this device sends: its own upload limit, and in a server, that server's limit too.
pub fn upload_limit(mine: &SizeLimits, server: Option<u64>) -> u64 {
    server.map_or(mine.max_upload, |s| s.min(mine.max_upload)).min(MAX_FILE_SIZE_LIMIT)
}

/// The largest file this device fetches on arrival: its own download limit, the server's limit when the file came in a
/// server (an older client may still post a larger one), and the automatic-download setting.
pub fn auto_fetch_limit(mine: &SizeLimits, server: Option<u64>, auto_accept: u64) -> u64 {
    server.map_or(mine.max_download, |s| s.min(mine.max_download)).min(auto_accept)
}

/// Why a file of `size` is over a limit for this device, for its card: this device's download limit first, then the
/// server's. `None` when it is within both.
pub fn over_limit_note(mine: &SizeLimits, server: Option<u64>, size: u64) -> Option<String> {
    if size > mine.max_download {
        Some(format!("Larger than your download limit ({})", dissent_files::size_text(mine.max_download)))
    } else {
        server
            .filter(|s| size > *s)
            .map(|s| format!("Larger than this server allows ({})", dissent_files::size_text(s)))
    }
}

/// The error sending a file of `size` over `limit` fails with. `server` says whether the server's limit is the one hit.
pub fn upload_refusal(name: &str, size: u64, limit: u64, server: bool) -> CoreError {
    let whose = if server { "this server's limit" } else { "your upload limit" };
    CoreError::Invalid(format!(
        "{name} is {}, larger than {whose} of {}.",
        dissent_files::size_text(size),
        dissent_files::size_text(limit)
    ))
}

impl Engine {
    /// This device's limits.
    pub fn size_limits(&self) -> SizeLimits {
        self.read().size_limits
    }

    /// Sets this device's limits. Each must be 1 MiB to 1 TiB.
    pub fn set_size_limits(&self, limits: SizeLimits) -> Result<SizeLimits> {
        if !valid_limit(limits.max_upload) || !valid_limit(limits.max_download) {
            return Err(CoreError::Invalid("a size limit is 1 MB to 1 TB".into()));
        }
        let mut w = self.write();
        w.paths.write_bin(&w.paths.size_limits(), &limits)?;
        w.size_limits = limits;
        Ok(limits)
    }

    /// The largest file members may share in `community`.
    pub(crate) fn server_file_size_limit(&self, community: dissent_identity::CommunityId) -> Option<u64> {
        self.read().communities.get(&community).map(|c| c.state().max_file_size())
    }

    /// Sets the largest file members may share in `community`. Needs `MANAGE_SERVER`.
    pub async fn set_server_file_size_limit(&self, community: dissent_identity::CommunityId, bytes: u64) -> Result<()> {
        if !valid_limit(bytes) {
            return Err(CoreError::Invalid("a size limit is 1 MB to 1 TB".into()));
        }
        self.author_publish(community, dissent_events::ChannelId::CONTROL, dissent_events::Payload::ServerFileSizeLimit { bytes })
            .await?;
        Ok(())
    }

    /// The largest file this device may send, in `community` when given.
    pub(crate) fn upload_limit_in(&self, community: Option<dissent_identity::CommunityId>) -> u64 {
        upload_limit(&self.size_limits(), community.and_then(|c| self.server_file_size_limit(c)))
    }

    /// Refuses a file of `size` named `name` that is over what this device may send, in `community` when given.
    pub(crate) fn check_upload(&self, community: Option<dissent_identity::CommunityId>, name: &str, size: u64) -> Result<()> {
        let mine = self.size_limits();
        let server = community.and_then(|c| self.server_file_size_limit(c));
        let limit = upload_limit(&mine, server);
        if size > limit {
            return Err(upload_refusal(name, size, limit, server.is_some_and(|s| s < mine.max_upload)));
        }
        Ok(())
    }

    /// Refuses a download of `size` over this device's download limit, unless the person asked to go ahead anyway.
    pub(crate) fn check_download(&self, size: u64, anyway: bool) -> Result<()> {
        let limit = self.size_limits().max_download;
        if !anyway && size > limit {
            return Err(CoreError::Invalid(format!(
                "{OVER_DOWNLOAD_LIMIT}: Larger than your download limit ({})",
                dissent_files::size_text(limit)
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    #[test]
    fn limits_run_from_one_mib_to_the_format_ceiling() {
        assert!(!valid_limit(MIN_FILE_SIZE_LIMIT - 1));
        assert!(valid_limit(MIN_FILE_SIZE_LIMIT));
        assert!(valid_limit(500 * GB), "above the old 10 GiB cap");
        assert!(valid_limit(MAX_FILE_SIZE_LIMIT));
        assert!(!valid_limit(MAX_FILE_SIZE_LIMIT + 1));
        assert_eq!(MAX_FILE_SIZE_LIMIT, dissent_files::MAX_FILE_SIZE, "the setting's ceiling is the format's");
        assert_eq!(SizeLimits::default(), SizeLimits { max_upload: 10 * GB, max_download: 10 * GB });
    }

    #[test]
    fn sending_keeps_to_the_smaller_of_mine_and_the_servers() {
        let mine = SizeLimits { max_upload: 20 * GB, max_download: 10 * GB };
        assert_eq!(upload_limit(&mine, None), 20 * GB, "a direct message: mine alone");
        assert_eq!(upload_limit(&mine, Some(2 * GB)), 2 * GB);
        assert_eq!(upload_limit(&mine, Some(500 * GB)), 20 * GB);
    }

    #[test]
    fn auto_fetch_keeps_to_mine_the_servers_and_the_auto_setting() {
        let mine = SizeLimits { max_upload: GB, max_download: 4 * GB };
        assert_eq!(auto_fetch_limit(&mine, None, u64::MAX), 4 * GB);
        assert_eq!(auto_fetch_limit(&mine, Some(GB), u64::MAX), GB);
        assert_eq!(auto_fetch_limit(&mine, Some(8 * GB), 100), 100);
    }

    #[test]
    fn a_card_says_which_limit_a_file_is_over() {
        let mine = SizeLimits { max_upload: GB, max_download: 4 * GB };
        assert_eq!(over_limit_note(&mine, None, 4 * GB), None);
        assert_eq!(over_limit_note(&mine, None, 5 * GB).as_deref(), Some("Larger than your download limit (4 GB)"));
        assert_eq!(over_limit_note(&mine, Some(2 * GB), 3 * GB).as_deref(), Some("Larger than this server allows (2 GB)"));
        assert_eq!(
            upload_refusal("movie.mkv", 3 * GB, 2 * GB, true).to_string(),
            "invalid: movie.mkv is 3 GB, larger than this server's limit of 2 GB."
        );
    }
}
