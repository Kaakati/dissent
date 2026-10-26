//! Link previews: off by default, and a link to a private address gets no preview.


use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use dissent_core::test_support::map_preview_host;

use crate::common::*;

#[tokio::test(flavor = "multi_thread")]
async fn previews_are_off_by_default_and_fetch_nothing() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let community = rustaceans(&alice, &bob).await;
    let general = channel_id(&alice.engine, community, "general").expect("general channel");
    assert!(!alice.engine.link_previews());

    let sent = alice
        .engine
        .send_message(community, general, "see https://example.com/page".into())
        .await
        .expect("alice posts");
    assert!(sent.embeds.is_empty(), "no preview unless the sender turns them on");
    assert_eq!(sent.text, "see https://example.com/page");

    finish(vec![alice, bob]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_to_a_private_address_gets_no_preview() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let community = rustaceans(&alice, &bob).await;
    let general = channel_id(&alice.engine, community, "general").expect("general channel");
    alice.engine.set_link_previews(true).expect("previews on");

    // localhost resolves to a loopback address, which the preview fetch refuses.
    let sent = alice
        .engine
        .send_message(community, general, "local https://localhost/admin".into())
        .await
        .expect("alice posts");
    assert!(sent.embeds.is_empty(), "a private address gets no preview");
    assert_eq!(sent.text, "local https://localhost/admin");

    finish(vec![alice, bob]).await;
}

/// The picture a test site serves: 800 by 400, so the preview's picture is 640 by 320.
fn a_picture_png() -> Vec<u8> {
    let mut png = Vec::new();
    image::RgbaImage::from_pixel(800, 400, image::Rgba([10, 200, 30, 255]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("encode the picture");
    png
}

/// A site on loopback. Its page links a picture, and it counts every request it serves.
async fn a_site(hits: Arc<AtomicUsize>) -> SocketAddr {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let page = format!(
        r##"<html><head><title>Ignored</title><meta property="og:site_name" content="Example Site">
        <meta property="og:title" content="A test page"><meta property="og:description" content="Made in a test">
        <meta name="theme-color" content="#1a2b3c"><meta property="og:image" content="http://picture.test:{}/pic.png">
        </head></html>"##,
        addr.port()
    );
    let picture = a_picture_png();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let (page, picture, hits) = (page.clone(), picture.clone(), hits.clone());
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).into_owned();
                hits.fetch_add(1, Ordering::SeqCst);
                let (kind, body) = if head.starts_with("GET /pic.png") {
                    ("image/png", picture)
                } else {
                    ("text/html", page.into_bytes())
                };
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(&body).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    addr
}

#[tokio::test(flavor = "multi_thread")]
async fn a_preview_picture_reaches_the_recipient_and_the_site_sees_only_the_sender() {
    let alice = peer("alice").await;
    let bob = peer("bob").await;
    let community = rustaceans(&alice, &bob).await;
    let general = channel_id(&alice.engine, community, "general").expect("general channel");

    let hits = Arc::new(AtomicUsize::new(0));
    let addr = a_site(hits.clone()).await;
    map_preview_host("preview.test", addr);
    map_preview_host("picture.test", addr);
    alice.engine.set_link_previews(true).expect("previews on");

    let link = format!("see http://preview.test:{}/page", addr.port());
    let sent = alice
        .engine
        .send_message(community, general, link.clone())
        .await
        .expect("alice posts the link");
    assert_eq!(sent.embeds.len(), 1, "alice's preview of the page");
    assert_eq!(sent.embeds[0].embed.title, "A test page");
    assert_eq!(sent.embeds[0].embed.site, "Example Site");
    assert_eq!(sent.embeds[0].theme.as_deref(), Some("#1a2b3c"));
    assert_eq!((sent.embeds[0].image_w, sent.embeds[0].image_h), (Some(640), Some(320)));
    assert_eq!(hits.load(Ordering::SeqCst), 2, "alice fetched the page and its picture");

    // Bob gets the picture as a blob from alice, so the site sees no request from him.
    eventually("bob has the preview picture", || {
        bob.engine
            .channel_messages(community, general, 10)
            .iter()
            .find(|m| m.text == link)
            .and_then(|m| m.embeds.first())
            .and_then(|e| e.image.clone())
            .map(|_| ())
    })
    .await;
    let copy = bob
        .engine
        .channel_messages(community, general, 10)
        .into_iter()
        .find(|m| m.text == link)
        .expect("bob's copy");
    let preview = &copy.embeds[0];
    assert_eq!(preview.embed.title, "A test page");
    assert_eq!((preview.image_w, preview.image_h), (Some(640), Some(320)));
    let picture = image::open(preview.image.as_ref().expect("the picture path")).expect("the picture decodes");
    assert_eq!((picture.width(), picture.height()), (640, 320));
    assert_eq!(hits.load(Ordering::SeqCst), 2, "bob never contacted the site");

    finish(vec![alice, bob]).await;
}

