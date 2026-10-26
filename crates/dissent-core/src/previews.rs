//! Link previews, made by the sender only.
//!
//! Fetching a URL shows the site the sender's address, so previews are off until the sender turns
//! them on. When the sender sends a message with links and previews are on, its device fetches each
//! `https` link once, and reads the Open Graph title, description, site name, theme colour and
//! picture. The picture is re-encoded as a small JPEG and stored as its own blob. The result travels
//! inside the sealed message. Recipients never fetch anything from the site.
//!
//! The fetch follows the push relay's rules. It refuses any host that resolves to a private,
//! loopback, link-local or other non-public address. It connects to an address it checked, and it
//! does not follow redirects. It reads at most 512 KB of page, and at most 5 MB of picture, with a 5 s
//! timeout for each request. The whole message's previews share one budget, and a picture that is not
//! in time is left out, so the message still goes out on time.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use dissent_files::{make_preview_image, Input, Thumbnail};
use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;
use tokio::time::Instant;

use crate::attachments::{EmbedExtra, EmbedImage};
use crate::engine::Engine;
use crate::error::Result;
use crate::store::{read_bin, write_bin};
use crate::world::World;

/// Most previews one message carries.
pub const MAX_EMBEDS: usize = 3;
/// Most bytes read from a page.
pub const MAX_PAGE_BYTES: usize = 512 * 1024;
/// Most bytes read from a link's picture. A larger one is dropped, not cut short.
pub const MAX_PICTURE_BYTES: usize = 5 * 1024 * 1024;
/// Time allowed for one page.
pub const PREVIEW_TIMEOUT: Duration = Duration::from_secs(5);
/// Time allowed for all previews of one message, together.
pub const PREVIEW_BUDGET: Duration = Duration::from_secs(3);

fn scheme_allowed(scheme: &str) -> bool {
    scheme == "https" || (cfg!(any(test, feature = "test-hooks")) && scheme == "http")
}

/// Test hook: maps a host to a loopback server, so the success path can run without the internet.
/// Only the tests set it, and only then the public-address check is skipped for that host. Built
/// only for tests, or with the `test-hooks` feature that the crate's own integration tests enable.
#[cfg(any(test, feature = "test-hooks"))]
pub(crate) static TEST_HOSTS: std::sync::Mutex<Vec<(String, SocketAddr)>> = std::sync::Mutex::new(Vec::new());

/// Maps `host` to the loopback server at `addr`. For tests only; see [`TEST_HOSTS`].
#[cfg(feature = "test-hooks")]
pub fn map_preview_host(host: &str, addr: SocketAddr) {
    TEST_HOSTS.lock().unwrap().push((host.to_string(), addr));
}

fn test_resolve(host: &str) -> Option<SocketAddr> {
    #[cfg(any(test, feature = "test-hooks"))]
    {
        TEST_HOSTS
            .lock()
            .ok()?
            .iter()
            .find(|(h, _)| h == host)
            .map(|(_, a)| *a)
    }
    #[cfg(not(any(test, feature = "test-hooks")))]
    {
        let _ = host;
        None
    }
}

/// A link preview, as sealed in a message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Embed {
    pub url: String,
    pub site: String,
    pub title: String,
    pub description: Option<String>,
}

/// The `https` links in a message, at most `MAX_EMBEDS`, without repeats.
pub(crate) fn links_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for token in text.split_whitespace() {
        let Some((scheme, _)) = token.split_once("://") else {
            continue;
        };
        if !scheme_allowed(scheme) {
            continue;
        }
        let url = token.trim_end_matches(|c: char| ".,;:!?)]}\"'".contains(c));
        if url.len() > scheme.len() + "://".len() && !out.iter().any(|u| u == url) {
            out.push(url.to_string());
            if out.len() == MAX_EMBEDS {
                break;
            }
        }
    }
    out
}

/// True for addresses a public host can have. The same ranges the push relay refuses.
pub fn ip_is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => ipv4_is_public(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(mapped) => ipv4_is_public(mapped),
            None => ipv6_is_public(v6),
        },
    }
}

fn ipv4_is_public(ip: Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    let shared = a == 100 && (64..128).contains(&b);
    let documentation = matches!(ip.octets(), [192, 0, 2, _] | [198, 51, 100, _] | [203, 0, 113, _]);
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_unspecified()
        || ip.is_multicast()
        || shared
        || documentation
        || a == 0)
}

fn ipv6_is_public(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    let link_local = first & 0xffc0 == 0xfe80;
    let unique_local = first & 0xfe00 == 0xfc00;
    let documentation = first == 0x2001 && ip.segments()[1] == 0x0db8;
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || link_local
        || unique_local
        || documentation)
}

/// What a page says about itself.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct PageMeta {
    pub title: Option<String>,
    pub description: Option<String>,
    pub site_name: Option<String>,
    pub theme: Option<[u8; 3]>,
    /// The picture as the page writes it: `og:image`, else `twitter:image`. Not resolved yet.
    pub image: Option<String>,
}

/// Reads a page's meta tags and title. The title falls back to `<title>`.
pub(crate) fn parse_meta(html: &str) -> PageMeta {
    let mut og_title = None;
    let mut og_desc = None;
    let mut meta_desc = None;
    let mut site_name = None;
    let mut theme = None;
    let mut og_image = None;
    let mut tw_image = None;
    let lower = html.to_ascii_lowercase();
    let mut at = 0;
    while let Some(found) = lower[at..].find("<meta") {
        let start = at + found;
        let end = match lower[start..].find('>') {
            Some(e) => start + e,
            None => break,
        };
        let tag = &html[start..end];
        let key = attr(tag, "property").or_else(|| attr(tag, "name")).map(|k| k.to_ascii_lowercase());
        let content = attr(tag, "content").map(|c| decode_entities(&c));
        match (key.as_deref(), content) {
            (Some("og:title"), Some(c)) if og_title.is_none() => og_title = Some(c),
            (Some("og:description"), Some(c)) if og_desc.is_none() => og_desc = Some(c),
            (Some("description"), Some(c)) if meta_desc.is_none() => meta_desc = Some(c),
            (Some("og:site_name"), Some(c)) if site_name.is_none() => site_name = Some(c),
            (Some("theme-color"), Some(c)) if theme.is_none() => theme = hex_color(&c),
            (Some("og:image"), Some(c)) if og_image.is_none() => og_image = Some(c),
            (Some("twitter:image"), Some(c)) if tw_image.is_none() => tw_image = Some(c),
            _ => {}
        }
        at = end;
    }
    let title = og_title.or_else(|| {
        let s = lower.find("<title")?;
        let open = s + lower[s..].find('>')? + 1;
        let close = open + lower[open..].find("</title")?;
        Some(decode_entities(html[open..close].trim()))
    });
    PageMeta {
        title: title.filter(|t| !t.trim().is_empty()),
        description: og_desc.or(meta_desc).filter(|d| !d.trim().is_empty()),
        site_name: site_name.filter(|s| !s.trim().is_empty()),
        theme,
        image: og_image.or(tw_image).filter(|i| !i.trim().is_empty()),
    }
}

/// A `#rgb` or `#rrggbb` colour, as three bytes.
fn hex_color(raw: &str) -> Option<[u8; 3]> {
    let hex = raw.trim().strip_prefix('#')?;
    let digits = hex.chars().map(|c| c.to_digit(16).map(|d| d as u8)).collect::<Option<Vec<u8>>>()?;
    match digits.as_slice() {
        [r, g, b] => Some([r * 17, g * 17, b * 17]),
        [r1, r2, g1, g2, b1, b2] => Some([r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2]),
        _ => None,
    }
}

/// The value of attribute `name` in an HTML tag, in either quote style.
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(pos) = lower[from..].find(name) {
        let at = from + pos;
        from = at + name.len();
        let before_ok = at == 0 || !lower.as_bytes()[at - 1].is_ascii_alphanumeric();
        let rest = tag[at + name.len()..].trim_start();
        if !before_ok || !rest.starts_with('=') {
            continue;
        }
        let rest = rest[1..].trim_start();
        let quote = rest.chars().next()?;
        if quote == '"' || quote == '\'' {
            let body = &rest[1..];
            return Some(body[..body.find(quote)?].to_string());
        }
    }
    None
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .trim()
        .to_string()
}

/// What a link's page says, and where its picture is.
pub(crate) struct Page {
    pub embed: Embed,
    /// The site's theme colour.
    pub theme: Option<[u8; 3]>,
    /// The picture's address, resolved against the page.
    pub picture: Option<reqwest::Url>,
}

/// A client that connects only to a public address it has checked for `url`'s host. `None` when the
/// scheme is not allowed, the host does not resolve, or any address it resolves to is not public.
async fn checked_client(url: &reqwest::Url) -> Option<reqwest::Client> {
    if !scheme_allowed(url.scheme()) {
        return None;
    }
    let host = url.host_str()?.to_string();
    let port = url.port_or_known_default().unwrap_or(443);
    let (addrs, trusted): (Vec<SocketAddr>, bool) = match test_resolve(&host) {
        Some(addr) => (vec![addr], true),
        None => (
            tokio::net::lookup_host((host.as_str(), port))
                .await
                .ok()?
                .collect(),
            false,
        ),
    };
    if addrs.is_empty() || (!trusted && addrs.iter().any(|a| !ip_is_public(a.ip()))) {
        return None;
    }
    reqwest::Client::builder()
        .timeout(PREVIEW_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .resolve(&host, addrs[0])
        .build()
        .ok()
}

/// The picture's address, resolved against the page's. Only the schemes previews may use are kept.
fn picture_url(page: &reqwest::Url, raw: &str) -> Option<reqwest::Url> {
    page.join(raw.trim()).ok().filter(|u| scheme_allowed(u.scheme()))
}

/// Fetches one link's page and reads its preview. `None` when the link is refused, unreachable, or
/// has no title.
pub(crate) async fn fetch_page(url: &str) -> Option<Page> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let client = checked_client(&parsed).await?;
    let mut resp = client
        .get(parsed.clone())
        .header(reqwest::header::ACCEPT, "text/html")
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let is_html = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.to_ascii_lowercase().starts_with("text/html"));
    if !is_html {
        return None;
    }
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await.ok()? {
        let room = MAX_PAGE_BYTES - body.len();
        body.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if body.len() >= MAX_PAGE_BYTES {
            break;
        }
    }
    let html = String::from_utf8_lossy(&body);
    let meta = parse_meta(&html);
    let host = parsed.host_str()?.to_string();
    Some(Page {
        embed: Embed {
            url: url.to_string(),
            site: meta.site_name.unwrap_or(host),
            title: meta.title?,
            description: meta.description,
        },
        theme: meta.theme,
        picture: meta.image.and_then(|raw| picture_url(&parsed, &raw)),
    })
}

/// Fetches a link's picture and re-encodes it as a preview picture. It is refused unless it is an
/// `image/*` response of at most `MAX_PICTURE_BYTES`, from a public address, without redirects.
pub(crate) async fn fetch_picture(url: &reqwest::Url) -> Option<Thumbnail> {
    let client = checked_client(url).await?;
    let mut resp = client
        .get(url.clone())
        .header(reqwest::header::ACCEPT, "image/*")
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() || resp.content_length().is_some_and(|n| n > MAX_PICTURE_BYTES as u64) {
        return None;
    }
    let is_image = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.to_ascii_lowercase().starts_with("image/"));
    if !is_image {
        return None;
    }
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await.ok()? {
        if body.len() + chunk.len() > MAX_PICTURE_BYTES {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    tokio::task::spawn_blocking(move || make_preview_image(&body))
        .await
        .ok()
        .flatten()
}

/// The results of `set` that arrive before `deadline`. Tasks still running at the deadline are aborted.
async fn join_until<T: Send + 'static>(set: &mut JoinSet<T>, deadline: Instant) -> Vec<T> {
    let mut out = Vec::new();
    loop {
        match tokio::time::timeout_at(deadline, set.join_next()).await {
            Ok(Some(Ok(v))) => out.push(v),
            Ok(Some(Err(_))) => {}
            Ok(None) => break,
            Err(_) => {
                set.abort_all();
                break;
            }
        }
    }
    out
}

/// Whether the one-time question about link previews has been asked on this device. Its own file, so
/// the attachment settings keep their layout.
fn prompted_path(w: &World) -> PathBuf {
    w.paths.local_state().with_file_name("link_preview_prompt.bin")
}

/// A link preview as the UI shows it: the sealed fields, plus the picture and colour this device has.
#[derive(Clone, Debug, Serialize)]
pub struct EmbedView {
    #[serde(flatten)]
    pub embed: Embed,
    /// The picture, as a local path, once it is on this device.
    pub image: Option<String>,
    pub image_w: Option<u32>,
    pub image_h: Option<u32>,
    /// The site's theme colour, as `#rrggbb`.
    pub theme: Option<String>,
}

impl Engine {
    /// Turns link previews on or off for messages this device sends. Off by default.
    pub fn set_link_previews(&self, on: bool) -> Result<()> {
        let mut guard = self.write();
        let w = &mut *guard;
        w.attach.link_previews = on;
        w.attach.save(&w.paths)
    }

    pub fn link_previews(&self) -> bool {
        self.read().attach.link_previews
    }

    /// Whether this device has asked the sender about link previews. The question is asked once.
    pub fn link_preview_prompted(&self) -> bool {
        read_bin::<bool>(&prompted_path(&self.read())).ok().flatten().unwrap_or(false)
    }

    /// Records that this device has asked the sender about link previews.
    pub fn mark_link_preview_prompted(&self) -> Result<()> {
        let path = prompted_path(&self.read());
        let _serial = self.write();
        write_bin(&path, &true)
    }

    /// Previews for the links in `text`, when the sender has them on, and the pictures that arrived in
    /// time, stored as blobs. Both are empty otherwise. Nothing is fetched when they are off.
    pub(crate) async fn build_embeds(&self, text: &str) -> (Vec<Embed>, Vec<EmbedExtra>) {
        if !self.link_previews() {
            return (Vec::new(), Vec::new());
        }
        // Every page is fetched at once. Whatever has not arrived by the budget is left out, so the
        // message goes out on time.
        let deadline = Instant::now() + PREVIEW_BUDGET;
        let mut pages = JoinSet::new();
        for (i, url) in links_in(text).into_iter().enumerate() {
            pages.spawn(async move { (i, fetch_page(&url).await) });
        }
        let mut found: Vec<(usize, Page)> = join_until(&mut pages, deadline)
            .await
            .into_iter()
            .filter_map(|(i, page)| page.map(|p| (i, p)))
            .collect();
        found.sort_by_key(|(i, _)| *i);
        // Pictures are fetched once the pages are known, within the same budget.
        let mut picts = JoinSet::new();
        for (i, page) in &found {
            if let Some(url) = page.picture.clone() {
                let i = *i;
                picts.spawn(async move { (i, fetch_picture(&url).await) });
            }
        }
        let mut pictures: BTreeMap<usize, Thumbnail> = join_until(&mut picts, deadline)
            .await
            .into_iter()
            .filter_map(|(i, t)| t.map(|t| (i, t)))
            .collect();
        let mut embeds = Vec::new();
        let mut extras = Vec::new();
        for (n, (i, page)) in found.into_iter().enumerate() {
            let image = match pictures.remove(&i) {
                Some(picture) => self.store_picture(picture).await,
                None => None,
            };
            if image.is_some() || page.theme.is_some() {
                extras.push(EmbedExtra {
                    index: n as u32,
                    image,
                    theme: page.theme,
                });
            }
            embeds.push(page.embed);
        }
        (embeds, extras)
    }

    /// Stores a picture as a blob for a message to carry. `None` when the store refuses it.
    async fn store_picture(&self, picture: Thumbnail) -> Option<EmbedImage> {
        let file = self
            .inner
            .files
            .add(Input::Bytes {
                name: "preview.jpg".into(),
                mime: "image/jpeg".into(),
                data: picture.jpeg.clone(),
            })
            .await
            .ok()?;
        // The sender sees its own preview too, so it keeps the picture the way a receiver does.
        let _ = crate::attachments::keep_picture(&self.read().paths, &file, &picture.jpeg);
        Some(EmbedImage {
            file,
            width: picture.width,
            height: picture.height,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_links_are_previewed_and_at_most_three() {
        // Under test, http is allowed for the mapped test hosts, so the plain-text case uses ftp here.
        let text = "see https://a.example/x, ftp://plain.example and https://b.example/y. https://c.example https://d.example";
        assert_eq!(
            links_in(text),
            vec!["https://a.example/x", "https://b.example/y", "https://c.example"]
        );
    }

    #[test]
    fn private_and_special_addresses_are_refused() {
        for ip in ["127.0.0.1", "10.1.2.3", "192.168.0.5", "172.16.9.9", "169.254.169.254", "100.64.1.1", "0.0.0.0", "::1", "fc00::1", "fe80::1", "::ffff:10.0.0.1"] {
            assert!(!ip_is_public(ip.parse().unwrap()), "{ip} must be refused");
        }
        for ip in ["93.184.216.34", "2606:2800:220:1::1"] {
            assert!(ip_is_public(ip.parse().unwrap()), "{ip} is public");
        }
    }

    #[test]
    fn open_graph_title_and_description_are_read() {
        let html = r#"<html><head><meta property="og:title" content="Hello &amp; welcome">
            <meta content='A page' name="description"><title>Ignored</title></head></html>"#;
        let meta = parse_meta(html);
        assert_eq!(meta.title.as_deref(), Some("Hello & welcome"));
        assert_eq!(meta.description.as_deref(), Some("A page"));
    }

    #[test]
    fn falls_back_to_the_title_element() {
        let meta = parse_meta("<title> Plain page </title>");
        assert_eq!(meta.title.as_deref(), Some("Plain page"));
        assert_eq!(meta.description, None);
    }

    #[test]
    fn the_picture_site_name_and_theme_colour_are_read() {
        let html = r##"<meta property="og:site_name" content="Example Site">
            <meta name="theme-color" content="#1a2b3c"><meta property="og:image" content="/pic.png">
            <meta name="twitter:image" content="/other.png">"##;
        let meta = parse_meta(html);
        assert_eq!(meta.site_name.as_deref(), Some("Example Site"));
        assert_eq!(meta.theme, Some([0x1a, 0x2b, 0x3c]));
        assert_eq!(meta.image.as_deref(), Some("/pic.png"), "og:image wins over twitter:image");
    }

    #[test]
    fn twitter_image_is_the_fallback_picture() {
        let meta = parse_meta(r#"<meta name="twitter:image" content="https://cdn.example/t.jpg">"#);
        assert_eq!(meta.image.as_deref(), Some("https://cdn.example/t.jpg"));
    }

    #[test]
    fn a_short_theme_colour_is_expanded_and_a_bad_one_is_ignored() {
        assert_eq!(hex_color("#abc"), Some([0xaa, 0xbb, 0xcc]));
        assert_eq!(hex_color("#AB12EF"), Some([0xab, 0x12, 0xef]));
        assert_eq!(hex_color("#zzz"), None);
        assert_eq!(hex_color("red"), None);
        assert_eq!(hex_color("#12345"), None);
    }

    #[test]
    fn picture_addresses_resolve_against_the_page_and_keep_only_allowed_schemes() {
        let page = reqwest::Url::parse("https://a.example/x/y").unwrap();
        assert_eq!(picture_url(&page, "/p.png").unwrap().as_str(), "https://a.example/p.png");
        assert_eq!(picture_url(&page, "//cdn.example/p.png").unwrap().as_str(), "https://cdn.example/p.png");
        assert!(picture_url(&page, "ftp://a.example/p.png").is_none());
    }
}

#[cfg(test)]
mod server_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Serves one canned response on loopback, and returns its address.
    async fn serve_once(response: String) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(response.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        });
        addr
    }

    /// Serves one canned response of raw bytes on loopback, and returns its address.
    async fn serve_once_bytes(response: Vec<u8>) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(&response).await;
                let _ = sock.shutdown().await;
            }
        });
        addr
    }

    fn html_response(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn map_host(host: &str, addr: SocketAddr) {
        TEST_HOSTS.lock().unwrap().push((host.to_string(), addr));
    }

    #[tokio::test]
    async fn a_mapped_page_yields_its_title_and_description() {
        let body = r#"<meta property="og:title" content="Loopback page"><meta property="og:description" content="Made in a test">"#;
        let addr = serve_once(html_response(body)).await;
        map_host("preview-ok.test", addr);
        let embed = fetch_page(&format!("http://preview-ok.test:{}/page", addr.port()))
            .await
            .expect("a preview")
            .embed;
        assert_eq!(embed.title, "Loopback page");
        assert_eq!(embed.description.as_deref(), Some("Made in a test"));
        assert_eq!(embed.site, "preview-ok.test");
    }

    #[tokio::test]
    async fn redirects_are_not_followed() {
        let response = "HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest/meta-data/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string();
        let addr = serve_once(response).await;
        map_host("preview-redirect.test", addr);
        let embed = fetch_page(&format!("http://preview-redirect.test:{}/", addr.port())).await;
        assert!(embed.is_none(), "a redirect is not followed, so there is no preview");
    }

    fn picture_response(bytes: &[u8], content_type: &str) -> Vec<u8> {
        let mut out = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        )
        .into_bytes();
        out.extend_from_slice(bytes);
        out
    }

    #[tokio::test]
    async fn a_picture_is_re_encoded_as_a_preview_picture() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(800, 400, image::Rgba([10, 200, 30, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let addr = serve_once_bytes(picture_response(&png, "image/png")).await;
        map_host("picture-ok.test", addr);
        let url = reqwest::Url::parse(&format!("http://picture-ok.test:{}/p.png", addr.port())).unwrap();
        let picture = fetch_picture(&url).await.expect("a picture");
        assert_eq!((picture.width, picture.height), (640, 320));
        assert!(dissent_files::is_preview_image(&picture.jpeg));
    }

    #[tokio::test]
    async fn a_picture_over_the_cap_is_dropped() {
        // One byte over the cap. The content is not a picture at all, so only the cap can refuse it.
        let body = vec![0u8; MAX_PICTURE_BYTES + 1];
        let addr = serve_once_bytes(picture_response(&body, "image/png")).await;
        map_host("picture-big.test", addr);
        let url = reqwest::Url::parse(&format!("http://picture-big.test:{}/p.png", addr.port())).unwrap();
        assert!(fetch_picture(&url).await.is_none());
    }

    #[tokio::test]
    async fn a_response_that_is_not_a_picture_is_dropped() {
        let addr = serve_once(html_response("<p>not a picture</p>")).await;
        map_host("picture-html.test", addr);
        let url = reqwest::Url::parse(&format!("http://picture-html.test:{}/p.png", addr.port())).unwrap();
        assert!(fetch_picture(&url).await.is_none());
    }
}
