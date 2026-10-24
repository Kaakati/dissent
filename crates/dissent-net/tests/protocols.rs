use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use dissent_files::{Files, Input};
use dissent_identity::DeviceKey;
use dissent_net::{Node, NodeConfig, NetError, RelayChoice};
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, DynProtocolHandler, ProtocolHandler},
};

const ECHO_ALPN: &[u8] = b"test/echo/0";

/// Echoes the first stream back to the caller.
#[derive(Debug)]
struct Echo;

impl ProtocolHandler for Echo {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let (mut send, mut recv) = connection.accept_bi().await?;
        let data = recv.read_to_end(1024).await.map_err(AcceptError::from_err)?;
        send.write_all(&data).await.map_err(AcceptError::from_err)?;
        send.finish().map_err(AcceptError::from_err)?;
        connection.closed().await;
        Ok(())
    }
}

fn loopback(extra: Vec<(Vec<u8>, Arc<dyn DynProtocolHandler>)>) -> NodeConfig {
    NodeConfig {
        relay: RelayChoice::Disabled,
        loopback_only: true,
        extra_protocols: extra,
        ..Default::default()
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("dissent-net-{tag}-{nanos:x}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
async fn extra_echo_protocol_is_reachable_on_loopback() {
    let echo: Arc<dyn DynProtocolHandler> = Arc::new(Echo);
    let a = Node::spawn(&DeviceKey::generate(), loopback(vec![(ECHO_ALPN.to_vec(), echo)]))
        .await
        .expect("spawn a");
    let b = Node::spawn(&DeviceKey::generate(), loopback(vec![])).await.expect("spawn b");

    let conn = b
        .endpoint()
        .connect(a.addr().await.to_iroh().unwrap(), ECHO_ALPN)
        .await
        .expect("dial echo");
    let (mut send, mut recv) = conn.open_bi().await.expect("open stream");
    send.write_all(b"echo me").await.expect("write");
    send.finish().expect("finish");
    let reply = recv.read_to_end(1024).await.expect("read");
    assert_eq!(reply, b"echo me");
    conn.close(0u32.into(), b"done");

    b.shutdown().await.expect("shutdown b");
    a.shutdown().await.expect("shutdown a");
}

#[tokio::test]
async fn extra_protocol_cannot_reuse_a_reserved_alpn() {
    let echo: Arc<dyn DynProtocolHandler> = Arc::new(Echo);
    let err = Node::spawn(
        &DeviceKey::generate(),
        loopback(vec![(b"dissent/ping/0".to_vec(), echo.clone())]),
    )
    .await
    .err()
    .expect("reserved ALPN must fail");
    assert!(matches!(err, NetError::Config(_)), "{err:?}");

    let err = Node::spawn(
        &DeviceKey::generate(),
        loopback(vec![(ECHO_ALPN.to_vec(), echo.clone()), (ECHO_ALPN.to_vec(), echo)]),
    )
    .await
    .err()
    .expect("duplicate extra ALPN must fail");
    assert!(matches!(err, NetError::Config(_)), "{err:?}");
}

/// A file added on A is fetched by B through A's node, mounted with `dissent_files`.
#[tokio::test]
async fn dissent_files_fetch_through_node_endpoint() {
    let dir_a = temp_dir("files-a");
    let dir_b = temp_dir("files-b");
    let files_a = Files::open(dir_a.join("data")).await.expect("open a");
    let files_b = Files::open(dir_b.join("data")).await.expect("open b");

    let handler: Arc<dyn DynProtocolHandler> = Arc::new(files_a.protocol_handler());
    let a = Node::spawn(
        &DeviceKey::generate(),
        loopback(vec![(dissent_files::ALPN.to_vec(), handler)]),
    )
    .await
    .expect("spawn a");
    let b = Node::spawn(&DeviceKey::generate(), loopback(vec![])).await.expect("spawn b");

    let payload = b"hello from A, through the blob store".to_vec();
    let fref = files_a
        .add(Input::Bytes {
            name: "greeting.txt".into(),
            mime: "text/plain".into(),
            data: payload.clone(),
        })
        .await
        .expect("add");

    let from = [a.addr().await.to_iroh().unwrap()];
    let out_dir = dir_b.join("out");
    std::fs::create_dir_all(&out_dir).unwrap();
    let path = tokio::time::timeout(
        Duration::from_secs(30),
        files_b.fetch(b.endpoint(), &from, &fref, &out_dir, |_, _| {}),
    )
    .await
    .expect("fetch timed out")
    .expect("fetch");
    assert_eq!(std::fs::read(&path).unwrap(), payload);

    b.shutdown().await.expect("shutdown b");
    a.shutdown().await.expect("shutdown a");
    // Router shutdown already stopped A's store. B's store was never mounted, so stop it here.
    files_b.shutdown().await.expect("stop b store");
    let _ = std::fs::remove_dir_all(dir_a);
    let _ = std::fs::remove_dir_all(dir_b);
}
