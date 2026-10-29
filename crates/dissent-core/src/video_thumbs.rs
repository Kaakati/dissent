//! Thumbnails of video files: the first frame, decoded with `dissent-video`, made into the same small
//! JPEG an image gets. A video whose container or codec this build cannot read gets none, and is sent
//! as a plain file. See `docs/design/08-files.md`, "Video thumbnails".

use std::path::Path;

use dissent_files::{make_thumbnail_rgb, Thumbnail, THUMB_LONG_SIDE};
use tracing::debug;

/// Whether a file of this MIME type may have a video thumbnail.
pub(crate) fn is_video(mime: &str) -> bool {
    mime.starts_with("video/")
}

/// The thumbnail of the video at `path`, or `None` when it cannot be read. Decoding runs on a
/// blocking thread.
pub(crate) async fn video_thumbnail(path: &Path) -> Option<Thumbnail> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || match dissent_video::still::first_frame(&path, THUMB_LONG_SIDE) {
        Ok(still) => make_thumbnail_rgb(still.width, still.height, still.rgb),
        Err(e) => {
            debug!(error = %e, "no thumbnail for this video");
            None
        }
    })
    .await
    .ok()?
}
