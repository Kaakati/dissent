//! "Hide my IP through Tor": an HTTP CONNECT proxy on loopback that carries each tunnel through a SOCKS5 proxy, the
//! Tor the user runs, by host name. iroh's relay client and the engine's HTTP client speak only HTTP CONNECT, and
//! Tor's SOCKS port is open in every Tor install (9050 for tor, 9150 for Tor Browser), while its HTTP tunnel port
//! needs a torrc edit. So the node's `proxy_url` points here, and Tor resolves every name: nothing is resolved on
//! this device.
//!
//! The bridge takes `CONNECT host:port` only, and refuses anything else. A tunnel that Tor can't open is answered
//! with a 502, and nothing tries any other way: with Tor down, nothing gets out.
//!
//! Any process on this machine can reach a loopback port, so each request must carry the bridge's secret, made new at
//! each start, as `Proxy-Authorization: Basic`; the URLs `SocksBridge::url` hands out carry it as their password.
//! Their user name is the purpose (relay, previews, updates). The bridge passes it on to Tor as the SOCKS user name,
//! and Tor (`IsolateSOCKSAuth`, on by default) gives each a circuit of its own, so one exit cannot tie this device's
//! relay traffic to the sites it previews or to its update checks.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use url::Url;

use crate::error::{NetError, Result};

/// Longest request head the bridge reads, so a client cannot make it buffer without bound.
const MAX_HEAD: usize = 8 * 1024;
/// How long the SOCKS proxy may take to open a tunnel. Tor takes a few seconds on a cold circuit.
const OPEN_WAIT: Duration = Duration::from_secs(60);
/// The SOCKS password sent with each purpose's user name. Tor isolates on the pair; only the name differs.
const SOCKS_PASSWORD: &str = "dissent";

/// A running bridge. It stops when dropped.
#[derive(Debug)]
pub struct SocksBridge {
    addr: SocketAddr,
    secret: String,
    socks: SocketAddr,
    task: JoinHandle<()>,
}

impl SocksBridge {
    /// Listens on a free loopback port and carries every tunnel through the SOCKS5 proxy at `socks`.
    pub async fn spawn(socks: SocketAddr) -> Result<SocksBridge> {
        if !socks.ip().is_loopback() {
            // A SOCKS proxy elsewhere would see every connection this device makes, and the link to it would show the
            // device's IP, so only one on this machine is used.
            return Err(NetError::Config("the Tor SOCKS proxy must be on this machine (127.0.0.1)".into()));
        }
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .map_err(|e| NetError::Config(format!("the Tor bridge could not listen: {e}")))?;
        let addr = listener.local_addr().map_err(|e| NetError::Config(e.to_string()))?;
        let secret = data_encoding::HEXLOWER.encode(&rand::random::<[u8; 16]>());
        let expected = secret.clone();
        let task = tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let expected = expected.clone();
                tokio::spawn(async move {
                    if let Err(e) = serve(client, socks, &expected).await {
                        tracing::debug!(error = %e, "Tor bridge tunnel ended");
                    }
                });
            }
        });
        Ok(SocksBridge { addr, secret, socks, task })
    }

    /// The bridge's address for one `purpose` (relay, previews, updates), with the secret: for `NodeConfig::proxy_url`
    /// and every HTTP client. Each purpose gets its own Tor circuit.
    pub fn url(&self, purpose: &str) -> Url {
        let mut url: Url = format!("http://{}", self.addr).parse().expect("a loopback address is a URL");
        url.set_username(purpose).expect("an http URL takes a user name");
        url.set_password(Some(&self.secret)).expect("an http URL takes a password");
        url
    }

    /// The SOCKS proxy it carries tunnels through.
    pub fn socks(&self) -> SocketAddr {
        self.socks
    }
}

impl Drop for SocksBridge {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Whether the SOCKS proxy at `socks` opens a tunnel to `host:port` within `wait`: Tor is running and has built a
/// circuit. For the app's indicator and its start-up check.
pub async fn reachable(socks: SocketAddr, host: &str, port: u16, wait: Duration) -> std::result::Result<(), String> {
    match tokio::time::timeout(wait, socks5_connect(socks, host, port, "status")).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("Tor did not open a connection in time".into()),
    }
}

async fn serve(mut client: TcpStream, socks: SocketAddr, secret: &str) -> std::result::Result<(), String> {
    let head = read_head(&mut client).await?;
    let Some(purpose) = authorized(&head, secret) else {
        let _ = client
            .write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"dissent\"\r\nConnection: close\r\n\r\n")
            .await;
        return Err("no or wrong proxy secret".into());
    };
    let Some((host, port)) = connect_target(&head) else {
        let _ = client.write_all(b"HTTP/1.1 405 Method Not Allowed\r\nConnection: close\r\n\r\n").await;
        return Err("not a CONNECT request".into());
    };
    let mut upstream = match tokio::time::timeout(OPEN_WAIT, socks5_connect(socks, &host, port, &purpose)).await {
        Ok(Ok(stream)) => stream,
        failed => {
            let _ = client.write_all(b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n").await;
            return Err(failed.map_or_else(|_| "Tor timed out".into(), |r| r.err().unwrap_or_default()));
        }
    };
    client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await.map_err(|e| e.to_string())?;
    tokio::io::copy_bidirectional(&mut client, &mut upstream).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// The request head, up to and including the blank line. Reads byte by byte, so nothing after it is taken from the
/// tunnel.
async fn read_head(client: &mut TcpStream) -> std::result::Result<Vec<u8>, String> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() >= MAX_HEAD {
            return Err("request head too long".into());
        }
        match client.read(&mut byte).await {
            Ok(1) => head.push(byte[0]),
            Ok(_) => return Err("closed before the request ended".into()),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(head)
}

/// The purpose a request names, when its `Proxy-Authorization: Basic` carries the bridge's `secret` as the password.
/// The purpose is a short lowercase name; anything else is refused.
fn authorized(head: &[u8], secret: &str) -> Option<String> {
    let text = std::str::from_utf8(head).ok()?;
    let value = text.lines().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim().eq_ignore_ascii_case("proxy-authorization").then(|| value.trim())
    })?;
    let (scheme, encoded) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = data_encoding::BASE64.decode(encoded.trim().as_bytes()).ok()?;
    let (user, password) = std::str::from_utf8(&decoded).ok()?.split_once(':')?;
    let fine = !user.is_empty() && user.len() <= 32 && user.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    (fine && password == secret).then(|| user.to_string())
}

/// The host and port of a `CONNECT host:port HTTP/1.x` request. `None` for any other request.
fn connect_target(head: &[u8]) -> Option<(String, u16)> {
    let line = std::str::from_utf8(head).ok()?.lines().next()?;
    let mut parts = line.split(' ');
    if parts.next()? != "CONNECT" {
        return None;
    }
    let authority = parts.next()?;
    if !parts.next()?.starts_with("HTTP/1.") {
        return None;
    }
    let (host, port) = authority.rsplit_once(':')?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() || host.len() > 255 {
        return None;
    }
    Some((host.to_string(), port.parse().ok()?))
}

/// Opens a tunnel to `host:port` through the SOCKS5 proxy at `socks` (RFC 1928), logged in as `isolation` (RFC 1929),
/// which Tor reads as the circuit to use. The host goes to the proxy as a name, so the proxy resolves it.
async fn socks5_connect(socks: SocketAddr, host: &str, port: u16, isolation: &str) -> std::result::Result<TcpStream, String> {
    let mut stream = TcpStream::connect(socks).await.map_err(|e| format!("Tor is not running at {socks}: {e}"))?;
    stream.write_all(&[5, 1, 2]).await.map_err(|e| e.to_string())?;
    let mut choice = [0u8; 2];
    stream.read_exact(&mut choice).await.map_err(|e| e.to_string())?;
    if choice != [5, 2] {
        return Err("the SOCKS proxy does not take a user name, so it can't keep circuits apart".into());
    }
    let user = u8::try_from(isolation.len()).map_err(|_| "isolation name too long".to_string())?;
    let mut login = vec![1, user];
    login.extend_from_slice(isolation.as_bytes());
    login.push(SOCKS_PASSWORD.len() as u8);
    login.extend_from_slice(SOCKS_PASSWORD.as_bytes());
    stream.write_all(&login).await.map_err(|e| e.to_string())?;
    let mut accepted = [0u8; 2];
    stream.read_exact(&mut accepted).await.map_err(|e| e.to_string())?;
    if accepted[1] != 0 {
        return Err("the SOCKS proxy refused the login".into());
    }
    let name = host.as_bytes();
    let len = u8::try_from(name.len()).map_err(|_| "host name too long".to_string())?;
    let mut request = vec![5, 1, 0, 3, len];
    request.extend_from_slice(name);
    request.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&request).await.map_err(|e| e.to_string())?;
    let mut reply = [0u8; 4];
    stream.read_exact(&mut reply).await.map_err(|e| e.to_string())?;
    if reply[0] != 5 {
        return Err("not a SOCKS5 proxy".into());
    }
    if reply[1] != 0 {
        return Err(format!("Tor could not reach {host}:{port} (SOCKS reply {})", reply[1]));
    }
    // The bound address that follows, which nothing here needs.
    let skip = match reply[3] {
        1 => 4,
        4 => 16,
        3 => {
            let mut n = [0u8; 1];
            stream.read_exact(&mut n).await.map_err(|e| e.to_string())?;
            usize::from(n[0])
        }
        _ => return Err("a SOCKS reply this app does not read".into()),
    };
    let mut rest = vec![0u8; skip + 2];
    stream.read_exact(&mut rest).await.map_err(|e| e.to_string())?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn only_a_connect_request_names_a_target() {
        assert_eq!(connect_target(b"CONNECT relay.example.org:443 HTTP/1.1\r\n\r\n"), Some(("relay.example.org".into(), 443)));
        assert_eq!(connect_target(b"CONNECT [::1]:8443 HTTP/1.1\r\n\r\n"), Some(("::1".into(), 8443)));
        assert_eq!(connect_target(b"GET http://example.org/ HTTP/1.1\r\n\r\n"), None, "plain requests are refused");
        assert_eq!(connect_target(b"CONNECT example.org HTTP/1.1\r\n\r\n"), None, "no port");
        assert_eq!(connect_target(b"CONNECT example.org:443 SPDY\r\n\r\n"), None);
    }

    #[test]
    fn a_request_needs_the_secret_and_names_its_purpose() {
        let auth = |user: &str, pass: &str| {
            format!("CONNECT a.example:443 HTTP/1.1\r\nProxy-Authorization: Basic {}\r\n\r\n", data_encoding::BASE64.encode(format!("{user}:{pass}").as_bytes()))
        };
        assert_eq!(authorized(auth("relay", "s3cret").as_bytes(), "s3cret"), Some("relay".into()));
        assert_eq!(authorized(auth("relay", "guess").as_bytes(), "s3cret"), None, "the wrong secret");
        assert_eq!(authorized(b"CONNECT a.example:443 HTTP/1.1\r\n\r\n", "s3cret"), None, "no secret at all");
        assert_eq!(authorized(auth("Relay Two", "s3cret").as_bytes(), "s3cret"), None, "a purpose is a short lowercase name");
        let lower = format!("CONNECT a.example:443 HTTP/1.1\r\nproxy-authorization: basic {}\r\n\r\n", data_encoding::BASE64.encode(b"updates:s3cret"));
        assert_eq!(authorized(lower.as_bytes(), "s3cret"), Some("updates".into()), "header and scheme in any case");
    }

    /// A SOCKS5 proxy on loopback that records the user name and the host each tunnel asks for, and reaches an echo
    /// server in their place.
    async fn fake_socks(echo: SocketAddr) -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let asked = Arc::new(Mutex::new(Vec::new()));
        let log = asked.clone();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                let log = log.clone();
                tokio::spawn(async move {
                    let mut hello = [0u8; 3];
                    s.read_exact(&mut hello).await.unwrap();
                    assert_eq!(hello, [5, 1, 2], "a login is offered, for the circuit");
                    s.write_all(&[5, 2]).await.unwrap();
                    let mut len = [0u8; 2];
                    s.read_exact(&mut len).await.unwrap();
                    let mut user = vec![0u8; usize::from(len[1])];
                    s.read_exact(&mut user).await.unwrap();
                    let mut plen = [0u8; 1];
                    s.read_exact(&mut plen).await.unwrap();
                    let mut pass = vec![0u8; usize::from(plen[0])];
                    s.read_exact(&mut pass).await.unwrap();
                    s.write_all(&[1, 0]).await.unwrap();
                    let user = String::from_utf8(user).unwrap();
                    let mut head = [0u8; 5];
                    s.read_exact(&mut head).await.unwrap();
                    assert_eq!(head[3], 3, "the host comes as a name, never an address");
                    let mut name = vec![0u8; usize::from(head[4])];
                    s.read_exact(&mut name).await.unwrap();
                    let mut port = [0u8; 2];
                    s.read_exact(&mut port).await.unwrap();
                    log.lock().unwrap().push(format!("{user} {}:{}", String::from_utf8(name).unwrap(), u16::from_be_bytes(port)));
                    let mut up = TcpStream::connect(echo).await.unwrap();
                    s.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0]).await.unwrap();
                    let _ = tokio::io::copy_bidirectional(&mut s, &mut up).await;
                });
            }
        });
        (addr, asked)
    }

    async fn echo_server() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let (mut r, mut w) = s.split();
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                });
            }
        });
        addr
    }

    /// A CONNECT request through `bridge` for `purpose`, with the secret its URL carries.
    fn connect_request(bridge: &SocksBridge, purpose: &str, target: &str) -> Vec<u8> {
        let url = bridge.url(purpose);
        let login = data_encoding::BASE64.encode(format!("{}:{}", url.username(), url.password().unwrap()).as_bytes());
        format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: Basic {login}\r\n\r\n").into_bytes()
    }

    fn bridge_addr(bridge: &SocksBridge) -> SocketAddr {
        bridge.url("relay").socket_addrs(|| None).unwrap()[0]
    }

    #[tokio::test]
    async fn a_tunnel_goes_through_socks_by_name_on_its_purpose_s_circuit() {
        let echo = echo_server().await;
        let (socks, asked) = fake_socks(echo).await;
        let bridge = SocksBridge::spawn(socks).await.unwrap();
        let mut client = TcpStream::connect(bridge_addr(&bridge)).await.unwrap();
        client.write_all(&connect_request(&bridge, "relay", "relay.example.org:443")).await.unwrap();
        let mut answer = vec![0u8; 39];
        client.read_exact(&mut answer).await.unwrap();
        assert!(answer.starts_with(b"HTTP/1.1 200"), "{}", String::from_utf8_lossy(&answer));
        client.write_all(b"ping").await.unwrap();
        let mut back = [0u8; 4];
        client.read_exact(&mut back).await.unwrap();
        assert_eq!(&back, b"ping", "the tunnel carries the bytes");
        assert_eq!(*asked.lock().unwrap(), vec!["relay relay.example.org:443".to_string()], "Tor got the name, unresolved");
        let mut preview = TcpStream::connect(bridge_addr(&bridge)).await.unwrap();
        preview.write_all(&connect_request(&bridge, "previews", "site.example.org:443")).await.unwrap();
        let mut answer = vec![0u8; 39];
        preview.read_exact(&mut answer).await.unwrap();
        assert!(asked.lock().unwrap().contains(&"previews site.example.org:443".to_string()), "another purpose, another circuit");
        assert!(reachable(socks, "check.example.org", 443, Duration::from_secs(5)).await.is_ok());
    }

    #[tokio::test]
    async fn a_local_process_without_the_secret_is_refused() {
        let echo = echo_server().await;
        let (socks, asked) = fake_socks(echo).await;
        let bridge = SocksBridge::spawn(socks).await.unwrap();
        let mut client = TcpStream::connect(bridge_addr(&bridge)).await.unwrap();
        client.write_all(b"CONNECT relay.example.org:443 HTTP/1.1\r\n\r\n").await.unwrap();
        let mut answer = Vec::new();
        let _ = client.read_to_end(&mut answer).await;
        assert!(answer.starts_with(b"HTTP/1.1 407"), "{}", String::from_utf8_lossy(&answer));
        assert!(asked.lock().unwrap().is_empty(), "Tor was never asked");
        let other = SocksBridge::spawn(socks).await.unwrap();
        assert_ne!(other.url("relay").password(), bridge.url("relay").password(), "a new secret each start");
    }

    #[tokio::test]
    async fn with_tor_down_a_tunnel_is_refused_and_nothing_else_is_tried() {
        // A free port with nothing on it stands in for a Tor that isn't running.
        let free = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
        let bridge = SocksBridge::spawn(free).await.unwrap();
        let mut client = TcpStream::connect(bridge_addr(&bridge)).await.unwrap();
        client.write_all(&connect_request(&bridge, "relay", "relay.example.org:443")).await.unwrap();
        let mut answer = Vec::new();
        let _ = client.read_to_end(&mut answer).await;
        assert!(answer.starts_with(b"HTTP/1.1 502"), "{}", String::from_utf8_lossy(&answer));
        assert!(reachable(free, "relay.example.org", 443, Duration::from_secs(5)).await.is_err());
    }

    #[tokio::test]
    async fn a_socks_proxy_on_another_machine_is_refused() {
        let elsewhere: SocketAddr = "192.0.2.1:9050".parse().unwrap();
        assert!(SocksBridge::spawn(elsewhere).await.is_err());
    }
}
