//! A fixed port is bound on both address families, so port forwarding and firewall rules for it cover IPv6 too.

use std::net::{SocketAddr, UdpSocket};

use dissent_identity::DeviceKey;
use dissent_net::{Node, NodeConfig, RelayChoice};

/// A UDP port free on IPv4 now, and on IPv6 when this host has it.
fn free_port() -> u16 {
    let socket = UdpSocket::bind("0.0.0.0:0").expect("bind a probe socket");
    socket.local_addr().unwrap().port()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: binds all interfaces, run with scripts/fwtest.sh"]
async fn a_fixed_port_is_bound_on_ipv4_and_on_ipv6() {
    let port = free_port();
    let has_v6 = UdpSocket::bind(("::", port)).is_ok();
    let config = NodeConfig { relay: RelayChoice::Disabled, dht_lookup: false, bind_port: port, ..Default::default() };
    let node = Node::spawn(&DeviceKey::generate(), config).await.expect("spawn on a fixed port");
    let bound: Vec<SocketAddr> = node.endpoint().bound_sockets();
    assert!(bound.iter().any(|a| a.is_ipv4() && a.port() == port), "IPv4 on the fixed port: {bound:?}");
    if has_v6 {
        assert!(bound.iter().any(|a| a.is_ipv6() && a.port() == port), "IPv6 on the fixed port too: {bound:?}");
    } else {
        eprintln!("skipped the IPv6 check: this host has no IPv6");
    }
    node.shutdown().await.expect("shutdown");
}
