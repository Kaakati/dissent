//! Link previews, made by the sender only.
//!
//! Fetching a URL shows the site the sender's address, so previews are off until the sender turns
//! them on. When the sender sends a message with links and previews are on, its device fetches each
//! `https` link once, and reads the Open Graph title and description. The result travels inside
//! the sealed message. Recipients never fetch anything.
//!
//! The fetch follows the push relay's rules. It refuses any host that resolves to a private,
//! loopback, link-local or other non-public address. It connects to an address it checked, and it
//! does not follow redirects. It reads at most 512 KB, with a 5 s timeout.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::engine::Engine;
use crate::error::Result;

/// Most previews one message carries.
pub const MAX_EMBEDS: usize = 3;
/// Most bytes read from a page.
pub const MAX_PAGE_BYTES: usize = 512 * 1024;
/// Time allowed for one page.
pub const PREVIEW_TIMEOUT: Duration = Duration::from_secs(5);
/// Time allowed for all previews of one message, together.
pub const PREVIEW_BUDGET: Duration = Duration::from_secs(3);

fn scheme_allowed(scheme: &str) -> bool {
    scheme == "https" || (cfg!(test) && scheme == "http")
}

/// Test hook: maps a host to a loopback server, so the success path can run without the internet.
/// Only the tests set it, and only then the public-address check is skipped for that host.
#[cfg(test)]
pub(crate) static TEST_HOSTS: std::sync::Mutex<Vec<(String, SocketAddr)>> = std::sync::Mutex::new(Vec::new());

fn test_resolve(host: &str) -> Option<SocketAddr> {
    #[cfg(test)]
    {
        TEST_HOSTS
            .lock()
            .ok()?
            .iter()
            .find(|(h, _)| h == host)
            .map(|(_, a)| *a)
    }
    #[cfg(not(test))]
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
        if !token.starts_with("https://") {
            continue;
        }
        let url = token.trim_end_matches(|c: char| ".,;:!?)]}\"'".contains(c));
        if url.len() > "https://".len() && !out.iter().any(|u| u == url) {
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

/// Reads the Open Graph title and description from a page. Falls back to `<title>`.
pub(crate) fn parse_page(html: &str) -> (Option<String>, Option<String>) {
    let mut og_title = None;
    let mut og_desc = None;
    let mut meta_desc = None;
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
    (title.filter(|t| !t.trim().is_empty()), og_desc.or(meta_desc).filter(|d| !d.trim().is_empty()))
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

/// Fetches one link's preview. `None` when the link is refused, unreachable, or has no title.
pub(crate) async fn fetch_embed(url: &str) -> Option<Embed> {
    let parsed = reqwest::Url::parse(url).ok()?;
    if !scheme_allowed(parsed.scheme()) {
        return None;
    }
    let host = parsed.host_str()?.to_string();
    let port = parsed.port_or_known_default().unwrap_or(443);
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
    let client = reqwest::Client::builder()
        .timeout(PREVIEW_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .resolve(&host, addrs[0])
        .build()
        .ok()?;
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
    let (title, description) = parse_page(&html);
    Some(Embed {
        url: url.to_string(),
        site: host,
        title: title?,
        description,
    })
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

    /// Previews for the links in `text`, when the sender has them on. Empty otherwise. Nothing is
    /// fetched when they are off.
    pub(crate) async fn build_embeds(&self, text: &str) -> Vec<Embed> {
        if !self.link_previews() {
            return Vec::new();
        }
        // Every link is fetched at once. Whatever has not arrived by the budget is left out, so
        // the message goes out on time.
        let mut set = tokio::task::JoinSet::new();
        for (i, url) in links_in(text).into_iter().enumerate() {
            set.spawn(async move { (i, fetch_embed(&url).await) });
        }
        let deadline = tokio::time::Instant::now() + PREVIEW_BUDGET;
        let mut found: Vec<(usize, Embed)> = Vec::new();
        loop {
            match tokio::time::timeout_at(deadline, set.join_next()).await {
                Ok(Some(Ok((i, Some(e))))) => found.push((i, e)),
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(_) => {
                    set.abort_all();
                    break;
                }
            }
        }
        found.sort_by_key(|(i, _)| *i);
        found.into_iter().map(|(_, e)| e).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_links_are_previewed_and_at_most_three() {
        let text = "see https://a.example/x, http://plain.example and https://b.example/y. https://c.example https://d.example";
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
        let (title, desc) = parse_page(html);
        assert_eq!(title.as_deref(), Some("Hello & welcome"));
        assert_eq!(desc.as_deref(), Some("A page"));
    }

    #[test]
    fn falls_back_to_the_title_element() {
        let (title, desc) = parse_page("<title> Plain page </title>");
        assert_eq!(title.as_deref(), Some("Plain page"));
        assert_eq!(desc, None);
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
        let embed = fetch_embed(&format!("http://preview-ok.test:{}/page", addr.port()))
            .await
            .expect("a preview");
        assert_eq!(embed.title, "Loopback page");
        assert_eq!(embed.description.as_deref(), Some("Made in a test"));
        assert_eq!(embed.site, "preview-ok.test");
    }

    #[tokio::test]
    async fn redirects_are_not_followed() {
        let response = "HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest/meta-data/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string();
        let addr = serve_once(response).await;
        map_host("preview-redirect.test", addr);
        let embed = fetch_embed(&format!("http://preview-redirect.test:{}/", addr.port())).await;
        assert!(embed.is_none(), "a redirect is not followed, so there is no preview");
    }
}
