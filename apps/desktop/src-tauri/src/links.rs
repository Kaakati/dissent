//! External links. The webview cannot open anything outside the app by itself: its opener permissions stay off, it may
//! not navigate away from the app, and it may not open windows. A link the person confirmed goes through
//! [`open_external`], which opens it in the default browser, and only for `http`, `https` and `mailto`. Navigations and
//! new-window requests the UI did not catch are sent to the UI as [`EVENT_EXTERNAL_LINK`], so they ask first too.

use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Emitter, Manager, Url, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;
use tracing::warn;

/// A link the webview tried to open on its own. The UI asks before opening it.
pub const EVENT_EXTERNAL_LINK: &str = "dissent://external-link";

/// The schemes an external link may have.
const SCHEMES: [&str; 3] = ["http", "https", "mailto"];

/// `raw` as a link to open outside the app, or why not. It must be exactly a URL: no surrounding whitespace and no control
/// characters anywhere, which a parser would otherwise strip or encode. The scheme is compared after parsing, so
/// `JavaScript:` is `javascript`.
pub fn external_url(raw: &str) -> Result<Url, String> {
    if raw.chars().any(|c| c.is_control()) || raw.trim() != raw || raw.is_empty() {
        return Err("This link has hidden characters in it, so it was not opened.".into());
    }
    let url = Url::parse(raw).map_err(|_| "This is not a link that can be opened.".to_string())?;
    if !SCHEMES.contains(&url.scheme()) {
        return Err(format!("Links starting with {}: are not opened outside the app.", url.scheme()));
    }
    // A web link is written `scheme://host...`. A parser forgives `http:///host`, `https:host`, `https:\\host` and the
    // like by finding a host anyway; such a link is refused rather than guessed at.
    if url.scheme() != "mailto" {
        let after = raw.split_once(':').map_or("", |(_, rest)| rest);
        let authority = after.strip_prefix("//").and_then(|rest| rest.chars().next());
        if !authority.is_some_and(|c| c != '/' && c != '\\') {
            return Err("This link is not written the usual way, so it was not opened.".into());
        }
    }
    if url.scheme() != "mailto" && url.host_str().is_none_or(str::is_empty) {
        return Err("This link has no site in it, so it was not opened.".into());
    }
    Ok(url)
}

/// Opens `url` in the default browser or mail app, once the person confirmed it.
#[tauri::command]
pub fn open_external(app: AppHandle, url: String) -> Result<(), String> {
    let url = external_url(&url)?;
    app.opener().open_url(url.as_str(), None::<&str>).map_err(|e| e.to_string())
}

/// Whether the webview may load `url` itself: the app's own pages only.
pub fn is_app_url(url: &Url) -> bool {
    match url.scheme() {
        "tauri" => true,
        "http" | "https" if url.host_str() == Some("tauri.localhost") => true,
        // The dev server, in a debug build.
        "http" if cfg!(debug_assertions) && matches!(url.host_str(), Some("127.0.0.1" | "localhost")) && url.port() == Some(1420) => true,
        "about" => url.as_str() == "about:blank",
        _ => false,
    }
}

/// Hands a link the webview tried to open by itself to the UI, which asks first. Anything else is dropped.
fn ask_ui(app: &AppHandle, url: &Url) {
    if external_url(url.as_str()).is_ok() {
        if let Err(e) = app.emit(EVENT_EXTERNAL_LINK, url.as_str()) {
            warn!(error = %e, "could not pass a link to the UI");
        }
    }
}

/// Makes the main window from its config, so it gets the navigation and new-window guards. `tauri.conf.json` marks it
/// `"create": false` for this.
pub fn build_main_window(app: &AppHandle) -> tauri::Result<()> {
    if app.get_webview_window("main").is_some() {
        return Ok(());
    }
    let Some(config) = app.config().app.windows.iter().find(|w| w.label == "main").cloned() else {
        warn!("no main window in the config");
        return Ok(());
    };
    let (nav, popup) = (app.clone(), app.clone());
    WebviewWindowBuilder::from_config(app, &config)?
        .on_navigation(move |url| {
            let ok = is_app_url(url);
            if !ok {
                ask_ui(&nav, url);
            }
            ok
        })
        .on_new_window(move |url, _| {
            ask_ui(&popup, &url);
            NewWindowResponse::Deny
        })
        .build()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_and_mail_links_open() {
        for ok in ["https://example.com/a?b=c#d", "http://example.com", "HTTPS://Example.com/path", "mailto:someone@example.com", "https://xn--80ak6aa92e.com/"] {
            assert!(external_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            "JAVASCRIPT:alert(1)",
            "file:///C:/Windows/System32/calc.exe",
            "file://server/share",
            "dissent://invite/abc",
            "data:text/html,<script>alert(1)</script>",
            "vbscript:msgbox",
            "ms-settings:privacy",
            "steam://run/1",
            "about:blank",
            "https:",
            "https://",
            "http:///path",
            "https:///example.com",
            "https:example.com",
            "https:/example.com",
            "https:\\\\example.com",
            "https:/\\example.com",
            "https://\\example.com",
            "https:\\/example.com",
            "HTTPS:///example.com",
            "example.com",
            "",
        ] {
            assert!(external_url(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn hidden_characters_do_not_sneak_a_scheme_past() {
        for bad in [
            " javascript:alert(1)",
            "javascript:alert(1) ",
            "\tjavascript:alert(1)",
            "java\nscript:alert(1)",
            "java\tscript:alert(1)",
            "\u{0}https://example.com",
            "https://example.com\u{7}",
            "https://exa\rmple.com",
            "\u{1b}https://example.com",
            "https://example.com\n",
            " https://example.com",
        ] {
            assert!(external_url(bad).is_err(), "{bad:?} must be refused");
        }
    }

    /// Everything that looks the main window up (tray, close to tray, deep links, a second launch, notification clicks)
    /// expects it to exist once setup has begun. It is made in setup now, not from the config, so both must hold.
    #[test]
    fn the_main_window_is_made_first_thing_in_setup() {
        let config: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let main = config["app"]["windows"].as_array().unwrap().iter().find(|w| w["label"] == "main").unwrap();
        assert_eq!(main["create"], false, "made by build_main_window, with its guards, and only there");
        assert_eq!(main["visible"], false, "shown once the profile is open");
        let lib = include_str!("lib.rs");
        let setup = &lib[lib.find("fn setup(app: &mut tauri::App)").unwrap()..];
        let body = &setup[setup.find('{').unwrap() + 1..];
        let first = body.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("//")).unwrap();
        assert_eq!(first, "links::build_main_window(app.handle())?;");
        // The deep-link start runs after setup, so the window is there when a cold-start link is offered.
        assert!(lib.find("deep_link::start(app.handle())").unwrap() > lib.find("if let Err(e) = setup(app)").unwrap());
    }

    #[test]
    fn the_webview_stays_on_the_app() {
        let url = |s: &str| Url::parse(s).unwrap();
        assert!(is_app_url(&url("tauri://localhost/index.html")));
        assert!(is_app_url(&url("http://tauri.localhost/")));
        assert!(is_app_url(&url("about:blank")));
        assert!(!is_app_url(&url("https://example.com/")));
        assert!(!is_app_url(&url("http://tauri.localhost.example.com/")));
        assert!(!is_app_url(&url("file:///C:/")));
        assert!(!is_app_url(&url("about:srcdoc")));
    }
}
