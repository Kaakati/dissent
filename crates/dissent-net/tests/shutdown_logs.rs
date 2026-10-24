//! Shutdown must not log warnings. This file is its own test binary, so it can install a global
//! tracing subscriber without affecting the other tests.

use std::sync::{Arc, Mutex};

use dissent_identity::{CommunityKey, DeviceId, DeviceKey};
use dissent_net::{MediaHandler, MediaLink, Node, NodeConfig, RelayChoice, RpcHandler, TopicId};
use futures_lite::future::Boxed;
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, DynProtocolHandler, ProtocolHandler},
};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::{
    Layer,
    layer::{Context, SubscriberExt},
};

const ECHO_ALPN: &[u8] = b"test/shutdown-echo/0";

/// Records every WARN and ERROR event as `level target`.
#[derive(Clone, Default)]
struct WarnCapture(Arc<Mutex<Vec<String>>>);

impl<S: Subscriber> Layer<S> for WarnCapture {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let level = *event.metadata().level();
        if level <= Level::WARN {
            self.0.lock().unwrap().push(format!("{level} {}", event.metadata().target()));
        }
    }
}

struct Upper;

impl RpcHandler for Upper {
    fn handle(&self, _from: DeviceId, req: Vec<u8>) -> Boxed<Vec<u8>> {
        Box::pin(async move { req.to_ascii_uppercase() })
    }
}

#[derive(Default)]
struct Keep(Mutex<Vec<MediaLink>>);

impl MediaHandler for Keep {
    fn incoming(&self, _from: DeviceId, link: MediaLink) {
        self.0.lock().unwrap().push(link);
    }
}

#[derive(Debug)]
struct Echo;

impl ProtocolHandler for Echo {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        connection.closed().await;
        Ok(())
    }
}

fn config(media: Arc<Keep>, extra: Vec<(Vec<u8>, Arc<dyn DynProtocolHandler>)>) -> NodeConfig {
    NodeConfig {
        relay: RelayChoice::Disabled,
        loopback_only: true,
        rpc: Some(Arc::new(Upper)),
        media: Some(media),
        extra_protocols: extra,
        ..Default::default()
    }
}

#[tokio::test]
async fn shutdown_with_live_protocols_logs_no_warnings() {
    let capture = WarnCapture::default();
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(capture.clone()))
        .expect("global subscriber");

    let media_sink = Arc::new(Keep::default());
    let echo: Arc<dyn DynProtocolHandler> = Arc::new(Echo);
    let a = Node::spawn(
        &DeviceKey::generate(),
        config(media_sink.clone(), vec![(ECHO_ALPN.to_vec(), echo)]),
    )
    .await
    .expect("spawn a");
    let b = Node::spawn(&DeviceKey::generate(), config(Arc::new(Keep::default()), vec![]))
        .await
        .expect("spawn b");
    b.remember(&a.addr().await).expect("remember a");

    // Exercise every protocol so each handler has live work at shutdown.
    b.ping(&a.addr().await).await.expect("ping");
    assert_eq!(b.request(a.id(), b"ok".to_vec()).await.expect("rpc"), b"OK");
    let _media = b.open_media(a.id()).await.expect("media");
    let _extra = b
        .endpoint()
        .connect(a.addr().await.to_iroh().unwrap(), ECHO_ALPN)
        .await
        .expect("extra protocol");
    let topic = TopicId::for_channel(&CommunityKey::generate().id(), &[3; 32]);
    let (_tx_a, _rx_a) = a.subscribe(topic, vec![]).await.expect("subscribe a");
    let (_tx_b, _rx_b) = b.subscribe(topic, vec![a.id()]).await.expect("subscribe b");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    capture.0.lock().unwrap().clear();
    b.shutdown().await.expect("shutdown b");
    a.shutdown().await.expect("shutdown a");

    let warnings = capture.0.lock().unwrap().clone();
    assert!(warnings.is_empty(), "warn-level logs during shutdown: {warnings:?}");
}
