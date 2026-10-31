//! The `dimg` scheme: how the window loads the pictures the engine keeps. Avatars, server icons, banners, thumbnails and
//! link-preview pictures are sealed at rest, so the asset protocol would hand the window ciphertext. A `dimg` URL names
//! the sealed file; the handler asks the engine to open it (`Engine::open_picture`, which refuses anything outside the
//! picture folders) and answers with the bytes, marked not to be cached, so the webview keeps no plaintext copy on disk.

use std::path::PathBuf;

use dissent_core::{CoreError, Engine};
use tauri::http::{header, Request, Response, StatusCode};
use tauri::{AppHandle, Manager, Runtime};

use crate::commands::AppState;

/// The scheme's name. The CSP's `img-src` allows it.
pub const SCHEME: &str = "dimg";

/// The URL of a sealed picture at `path`. It matches Tauri's `convertFileSrc(path, "dimg")`.
pub fn picture_url(path: &str) -> String {
    let base = if cfg!(windows) { "http://dimg.localhost/" } else { "dimg://localhost/" };
    let mut out = String::from(base);
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The file path a `dimg` URL path names: percent-decoded, without the leading slash.
fn path_of(uri_path: &str) -> Option<PathBuf> {
    let raw = uri_path.strip_prefix('/').unwrap_or(uri_path).as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' {
            let hex = std::str::from_utf8(raw.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(raw[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok().map(PathBuf::from)
}

fn status(code: StatusCode) -> Response<Vec<u8>> {
    Response::builder().status(code).body(Vec::new()).unwrap_or_default()
}

/// Answers one `dimg` request.
pub async fn respond<R: Runtime>(app: &AppHandle<R>, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let state = app.state::<AppState>();
    let Ok(engine) = crate::commands::engine(&state).await else {
        return status(StatusCode::SERVICE_UNAVAILABLE);
    };
    answer(engine, request.uri().path()).await
}

/// The answer to a request for `uri_path`: the opened picture, 403 for a path outside the picture folders, 404 for a
/// picture that isn't there.
async fn answer(engine: Engine, uri_path: &str) -> Response<Vec<u8>> {
    let Some(path) = path_of(uri_path) else {
        return status(StatusCode::BAD_REQUEST);
    };
    match tauri::async_runtime::spawn_blocking(move || engine.open_picture(&path)).await {
        Ok(Ok((bytes, mime))) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime)
            // The webview's HTTP cache is on disk. A decrypted picture must not land there.
            .header(header::CACHE_CONTROL, "no-store")
            .body(bytes)
            .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR)),
        Ok(Err(CoreError::Invalid(_))) => status(StatusCode::FORBIDDEN),
        Ok(Err(CoreError::NotFound(_))) => status(StatusCode::NOT_FOUND),
        _ => status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn a_picture_url_names_its_file_and_reads_back() {
        let path = if cfg!(windows) { r"C:\Users\a b\avatars\x.png" } else { "/home/a b/avatars/x.png" };
        let url = picture_url(path);
        assert!(url.starts_with(if cfg!(windows) { "http://dimg.localhost/" } else { "dimg://localhost/" }));
        let rest = url.split_once("localhost").unwrap().1;
        assert_eq!(path_of(rest).unwrap(), PathBuf::from(path));
        assert!(path_of("/%zz").is_none(), "a bad escape is refused");
    }

    /// A 1x1 PNG.
    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

    /// The `dimg` URL path of a file, as the window asks for it.
    fn uri(path: &Path) -> String {
        let url = picture_url(&path.to_string_lossy());
        url.split_once("localhost").unwrap().1.to_string()
    }

    async fn ask(engine: &Engine, uri_path: &str) -> Response<Vec<u8>> {
        answer(engine.clone(), uri_path).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_scheme_serves_kept_pictures_and_nothing_else() {
        use base64::Engine as _;
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("dissent-dimg-{}-{nanos}", std::process::id()));
        let profile = dir.join("profile");
        std::fs::create_dir_all(&profile).unwrap();
        let net = dissent_core::NodeConfig {
            relay: dissent_core::RelayChoice::Disabled,
            loopback_only: true,
            dht_lookup: false,
            ..Default::default()
        };
        let (engine, _events) = Engine::start_with_options(
            dissent_identity::Identity::create(1_700_000_000_000),
            profile.clone(),
            net,
            dissent_core::test_support::vault_for(&profile),
            dissent_core::test_support::fast_timings(),
            None,
        )
        .await
        .expect("engine starts");

        let png = base64::engine::general_purpose::STANDARD.decode(PNG).unwrap();
        engine.set_avatar(png.clone()).await.expect("avatar set");
        let avatar = PathBuf::from(engine.me().avatar.expect("the avatar is kept"));
        assert_ne!(std::fs::read(&avatar).unwrap(), png, "the avatar is sealed at rest");

        // A kept picture comes back decrypted, typed, and marked not to be cached.
        let ok = ask(&engine, &uri(&avatar)).await;
        assert_eq!(ok.status(), StatusCode::OK);
        assert_eq!(ok.body(), &png);
        assert_eq!(ok.headers()[header::CONTENT_TYPE], "image/png");
        assert_eq!(ok.headers()[header::CACHE_CONTROL], "no-store");

        // So do the other folder's pictures, and a server's icon and banner: a photo's thumbnail is a JPEG.
        let server = engine.create_community("Pictures".into()).await.expect("server");
        let id: dissent_core::CommunityId = server.id.parse().unwrap();
        engine.set_community_icon(id, png.clone()).await.expect("icon");
        engine.set_community_banner(id, png.clone()).await.expect("banner");
        let view = engine.community_view(id).expect("the server opens");
        let general = view.channels.iter().find(|c| c.name == "general").expect("general").id.parse().unwrap();
        let photo = dir.join("photo.png");
        image::RgbImage::from_pixel(800, 400, image::Rgb([200, 40, 90])).save(&photo).unwrap();
        let sent = engine.send_files(id, general, vec![photo], String::new(), false).await.expect("photo sent");
        let thumb = PathBuf::from(sent.attachments[0].thumb.as_ref().and_then(|t| t.path.clone()).expect("own thumbnail kept"));
        assert_eq!(thumb.parent().and_then(|p| p.file_name()), Some(std::ffi::OsStr::new("thumbnails")));
        let thumb_answer = ask(&engine, &uri(&thumb)).await;
        assert_eq!(thumb_answer.status(), StatusCode::OK);
        assert_eq!(thumb_answer.headers()[header::CONTENT_TYPE], "image/jpeg");
        assert_eq!(thumb_answer.headers()[header::CACHE_CONTROL], "no-store");
        assert!(image::load_from_memory(thumb_answer.body()).is_ok(), "the thumbnail decodes");
        for picture in [view.icon.clone(), view.banner.clone()] {
            let answer = ask(&engine, &uri(Path::new(&picture.expect("the server picture is kept")))).await;
            assert_eq!((answer.status(), answer.body()), (StatusCode::OK, &png));
        }

        // A store file beside the picture folders, a `..` escape out of them, a relative path and a file elsewhere on
        // disk are all refused.
        let outside = dir.join("outside.png");
        std::fs::write(&outside, &png).unwrap();
        let store_file = std::fs::read_dir(&profile)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.is_file())
            .expect("the profile holds a store file");
        let avatars = avatar.parent().unwrap();
        let escape = avatars.join("..").join(store_file.file_name().unwrap());
        for (what, uri_path) in [
            ("a store file", uri(&store_file)),
            ("a `..` escape", uri(&escape)),
            ("a relative path", "/avatars%2Fx.png".to_string()),
            ("a file elsewhere", uri(&outside)),
        ] {
            assert_eq!(ask(&engine, &uri_path).await.status(), StatusCode::FORBIDDEN, "{what} is refused");
        }
        assert_eq!(ask(&engine, "/%zz").await.status(), StatusCode::BAD_REQUEST, "a bad escape is refused");

        // A link inside a picture folder that leads out of it is refused too. Windows needs developer mode or admin
        // rights to make one, so there the case runs only when it can.
        let link = avatars.join("link.png");
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&outside, &link).is_ok();
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_file(&outside, &link).is_ok();
        if linked {
            assert_eq!(ask(&engine, &uri(&link)).await.status(), StatusCode::FORBIDDEN, "a link out is refused");
        } else {
            eprintln!("no rights to make a symlink here; the link case runs on Unix");
        }

        // A picture that isn't there is a 404.
        let missing = ask(&engine, &uri(&avatars.join("missing.png"))).await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);

        engine.shutdown().await.expect("clean shutdown");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
