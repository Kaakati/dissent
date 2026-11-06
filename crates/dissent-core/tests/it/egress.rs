//! Where a device's traffic goes when it hides its IP. Through a proxy, every connection the device makes, to the
//! relay, the relay's latency probes and the sites of its link previews, goes through the proxy and names its host,
//! so nothing is resolved on the device, and no IP socket is bound at all. Behind a relay alone, the device fetches
//! no link preview, since the site would see the IP the relay hides. A local HTTP proxy stands in for Tor here.

use crate::common::*;
use crate::previews::a_site;
use crate::relay_calls::{spawn_relay, start_on_relay, wait_online, TestRelay};
use dissent_core::{Egress, Engine, Identity, NodeConfig, RelayChoice};
use iroh::tls::CaTlsConfig;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

/// An HTTP proxy on loopback that records each request's method and target, and reaches the hosts it is given. It
/// tunnels `CONNECT host:port`, and forwards a plain `GET http://host:port/path`, as an HTTP proxy does.
struct Proxy {
    url: Url,
    seen: Arc<Mutex<Vec<(String, String)>>>,
}

impl Proxy {
    async fn spawn(hosts: BTreeMap<String, SocketAddr>) -> Proxy {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind the proxy");
        let url: Url = format!("http://{}", listener.local_addr().expect("addr")).parse().expect("proxy url");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (log, hosts) = (seen.clone(), Arc::new(hosts));
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                tokio::spawn(serve(client, log.clone(), hosts.clone()));
            }
        });
        Proxy { url, seen }
    }

    fn requests(&self) -> Vec<(String, String)> {
        self.seen.lock().unwrap().clone()
    }
}

/// The address for `host:port`, from the proxy's own table: the proxy resolves names, never the device.
fn reach(hosts: &BTreeMap<String, SocketAddr>, authority: &str) -> Option<SocketAddr> {
    let (host, port) = authority.rsplit_once(':')?;
    let base = hosts.get(host)?;
    Some(SocketAddr::new(base.ip(), port.parse().ok()?))
}

async fn serve(mut client: tokio::net::TcpStream, log: Arc<Mutex<Vec<(String, String)>>>, hosts: Arc<BTreeMap<String, SocketAddr>>) {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match client.read(&mut byte).await {
            Ok(1) => head.push(byte[0]),
            _ => return,
        }
    }
    let text = String::from_utf8_lossy(&head).into_owned();
    let mut first = text.lines().next().unwrap_or_default().split(' ');
    let (method, target) = (first.next().unwrap_or_default().to_string(), first.next().unwrap_or_default().to_string());
    log.lock().unwrap().push((method.clone(), target.clone()));
    if method == "CONNECT" {
        let Some(addr) = reach(&hosts, &target) else { return };
        let Ok(mut upstream) = tokio::net::TcpStream::connect(addr).await else { return };
        if client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await.is_err() {
            return;
        }
        let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
        return;
    }
    // An absolute-form request: forward it to the host in its target, in origin form.
    let Some(rest) = target.strip_prefix("http://") else { return };
    let (authority, path) = rest.split_once('/').map_or((rest, "/".to_string()), |(a, p)| (a, format!("/{p}")));
    let Some(addr) = reach(&hosts, authority) else { return };
    let Ok(mut upstream) = tokio::net::TcpStream::connect(addr).await else { return };
    let forwarded = text.replacen(&target, &path, 1);
    if upstream.write_all(forwarded.as_bytes()).await.is_err() {
        return;
    }
    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
}

/// The test relay under the name `localhost`, which only the proxy resolves.
fn relay_by_name(relay: &TestRelay) -> Url {
    let mut url = relay.url.clone();
    url.set_host(Some("localhost")).expect("a host");
    url
}

/// An engine whose every connection goes through `proxy`, on `relay` by name.
async fn start_proxied(relay: &TestRelay, proxy: &Proxy, name: &str) -> Peer {
    let dir = scratch(name);
    let identity = Identity::create(now());
    let net = NodeConfig {
        relay: RelayChoice::Custom(vec![relay_by_name(relay)]),
        relay_ca: Some(CaTlsConfig::custom_roots(vec![relay.cert.clone()])),
        proxy_url: Some(proxy.url.clone()),
        portmapper: false,
        // The engine reads this too: it accepts a relay on loopback, as the test relay is.
        loopback_only: true,
        dht_lookup: false,
        ..NodeConfig::default()
    };
    assert_eq!(net.egress(), Egress::Proxied(proxy.url.clone()));
    let (engine, events) = Engine::start_with_timings(
        identity.clone(),
        dir.clone(),
        net,
        dissent_core::test_support::vault_for(&dir),
        dissent_core::test_support::fast_timings(),
    )
    .await
    .expect("engine starts");
    engine.set_display_name(name.to_string()).await.expect("name");
    Peer { engine, identity, dir, _events: events }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_proxied_device_reaches_its_relay_and_link_sites_only_through_the_proxy() {
    bounded(async {
        let relay = spawn_relay().await;
        let relay_port = relay.url.port().expect("a port");
        let hits = Arc::new(AtomicUsize::new(0));
        let site = a_site(hits.clone()).await;
        let hosts = BTreeMap::from([
            ("localhost".to_string(), SocketAddr::from(([127, 0, 0, 1], relay_port))),
            ("preview.test".to_string(), site),
            ("picture.test".to_string(), site),
        ]);
        let proxy = Proxy::spawn(hosts).await;
        let alice = start_proxied(&relay, &proxy, "alice-proxied").await;
        let bob = start_proxied(&relay, &proxy, "bob-proxied").await;
        wait_online(&[&alice, &bob]).await;

        for p in [&alice, &bob] {
            assert_eq!(dissent_core::test_support::bound_socket_count(&p.engine), 0, "no IP socket at all");
            assert!(!dissent_core::test_support::uses_dns_lookup(&p.engine), "no lookup through the system resolver");
        }
        let relay_target = format!("localhost:{relay_port}");
        let tunnels: Vec<String> = proxy.requests().into_iter().filter(|(m, _)| m == "CONNECT").map(|(_, t)| t).collect();
        assert!(tunnels.contains(&relay_target), "the relay was reached through the proxy: {tunnels:?}");
        assert!(
            tunnels.iter().all(|t| t.starts_with("localhost:")),
            "every tunnel names its host, so the device resolved nothing: {tunnels:?}"
        );

        // A link preview: the page and its picture through the proxy, by name.
        let community = rustaceans(&alice, &bob).await;
        let general = channel_id(&alice.engine, community, "general").expect("general");
        alice.engine.set_link_previews(true).expect("previews on");
        assert!(!alice.engine.link_previews_blocked(), "through a proxy, the site sees the proxy's exit");
        let link = format!("see http://preview.test:{}/page", site.port());
        let sent = alice.engine.send_message(community, general, link).await.expect("alice posts the link");
        assert_eq!(sent.embeds.len(), 1, "the preview came through the proxy");
        assert_eq!(sent.embeds[0].embed.title, "A test page");
        let fetched: Vec<String> = proxy.requests().into_iter().filter(|(m, _)| m == "GET").map(|(_, t)| t).collect();
        assert!(fetched.iter().any(|t| t.starts_with("http://preview.test:")), "the page, by name: {fetched:?}");
        assert!(fetched.iter().any(|t| t.starts_with("http://picture.test:")), "and its picture: {fetched:?}");
        assert_eq!(hits.load(Ordering::SeqCst), 2, "the site saw exactly the two fetches the proxy forwarded");
        finish(vec![alice, bob]).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_device_behind_a_relay_fetches_no_link_preview() {
    bounded(async {
        let relay = spawn_relay().await;
        let hits = Arc::new(AtomicUsize::new(0));
        let site = a_site(hits.clone()).await;
        dissent_core::test_support::map_preview_host("hidden-preview.test", site);
        let alice = start_on_relay(&relay, "alice-relayed").await;
        let bob = start_on_relay(&relay, "bob-relayed").await;
        wait_online(&[&alice, &bob]).await;
        for p in [&alice, &bob] {
            assert_eq!(dissent_core::test_support::bound_socket_count(&p.engine), 0, "no IP socket at all");
            assert!(!dissent_core::test_support::uses_dns_lookup(&p.engine), "no lookup through the system resolver");
        }

        let community = rustaceans(&alice, &bob).await;
        let general = channel_id(&alice.engine, community, "general").expect("general");
        alice.engine.set_link_previews(true).expect("previews on");
        assert!(alice.engine.link_previews_blocked(), "the site would see the IP the relay hides");
        let link = format!("see http://hidden-preview.test:{}/page", site.port());
        let sent = alice.engine.send_message(community, general, link).await.expect("alice posts the link");
        assert!(sent.embeds.is_empty(), "no preview while the IP is hidden");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(hits.load(Ordering::SeqCst), 0, "the site saw nothing");
        finish(vec![alice, bob]).await;
    })
    .await;
}

/// A SOCKS5 server on loopback standing in for Tor: it takes a user name and password (RFC 1929), records the user
/// name and the host each tunnel asks for, and reaches the hosts it is given.
async fn fake_tor(hosts: BTreeMap<String, SocketAddr>) -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let asked = Arc::new(Mutex::new(Vec::new()));
    let (log, hosts) = (asked.clone(), Arc::new(hosts));
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            let (log, hosts) = (log.clone(), hosts.clone());
            tokio::spawn(async move {
                let mut hello = [0u8; 3];
                if s.read_exact(&mut hello).await.is_err() {
                    return;
                }
                let _ = s.write_all(&[5, 2]).await;
                let mut len = [0u8; 2];
                let _ = s.read_exact(&mut len).await;
                let mut user = vec![0u8; usize::from(len[1])];
                let _ = s.read_exact(&mut user).await;
                let mut plen = [0u8; 1];
                let _ = s.read_exact(&mut plen).await;
                let mut pass = vec![0u8; usize::from(plen[0])];
                let _ = s.read_exact(&mut pass).await;
                let _ = s.write_all(&[1, 0]).await;
                let mut head = [0u8; 5];
                if s.read_exact(&mut head).await.is_err() || head[3] != 3 {
                    return;
                }
                let mut name = vec![0u8; usize::from(head[4])];
                let _ = s.read_exact(&mut name).await;
                let mut port = [0u8; 2];
                let _ = s.read_exact(&mut port).await;
                let (name, port) = (String::from_utf8_lossy(&name).into_owned(), u16::from_be_bytes(port));
                log.lock().unwrap().push(format!("{} {name}:{port}", String::from_utf8_lossy(&user)));
                let Some(base) = hosts.get(&name) else { return };
                let Ok(mut up) = tokio::net::TcpStream::connect(SocketAddr::new(base.ip(), port)).await else { return };
                let _ = s.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0]).await;
                let _ = tokio::io::copy_bidirectional(&mut s, &mut up).await;
            });
        }
    });
    (addr, asked)
}

/// Hide through Tor, end to end: the engine's relay connection goes through the Tor bridge, which needs its secret,
/// and reaches the relay through SOCKS by name, on the relay's own circuit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "networked: run with scripts/fwtest.sh"]
async fn a_device_hidden_through_tor_reaches_its_relay_through_the_bridge_on_its_own_circuit() {
    bounded(async {
        let relay = spawn_relay().await;
        let relay_port = relay.url.port().expect("a port");
        let (tor, asked) = fake_tor(BTreeMap::from([("localhost".to_string(), SocketAddr::from(([127, 0, 0, 1], relay_port)))])).await;
        let bridge = dissent_core::SocksBridge::spawn(tor).await.expect("the bridge starts");
        let proxy = Proxy { url: bridge.url("relay"), seen: Arc::new(Mutex::new(Vec::new())) };
        let alice = start_proxied(&relay, &proxy, "alice-tor").await;
        wait_online(&[&alice]).await;
        assert_eq!(dissent_core::test_support::bound_socket_count(&alice.engine), 0, "no IP socket at all");
        let tunnels = asked.lock().unwrap().clone();
        assert!(tunnels.contains(&format!("relay localhost:{relay_port}")), "the relay through Tor, by name, on its circuit: {tunnels:?}");
        assert!(tunnels.iter().all(|t| t.starts_with("relay ")), "the node used only the relay's circuit: {tunnels:?}");
        finish(vec![alice]).await;
    })
    .await;
}
