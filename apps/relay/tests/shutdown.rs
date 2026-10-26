//! The relay stops cleanly and promptly when it is sent SIGTERM, as a service manager sends it. It is
//! configured through environment variables, as a systemd unit configures it. Unix only: a Windows service
//! is stopped by the Service Control Manager, and that path is not run here.
#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// A loopback port that nothing listens on right now.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

#[test]
fn the_relay_exits_cleanly_on_sigterm_within_seconds() {
    let port = free_port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_dissent-relay"))
        .env("DISSENT_RELAY_DEV", "true")
        .env("DISSENT_RELAY_BIND", format!("127.0.0.1:{port}"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start the relay");

    // The relay prints its address on stdout once it is listening.
    let stdout = child.stdout.take().expect("stdout is piped");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) if line.contains("Relay listening") => break,
            Ok(_) => {}
            Err(_) => assert!(Instant::now() < deadline, "the relay did not start listening"),
        }
    }
    // The relay installs its signal handler once it is serving. Give it a moment after the banner.
    std::thread::sleep(Duration::from_millis(500));

    let asked = Instant::now();
    let status = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("send SIGTERM");
    assert!(status.success(), "kill -TERM failed");

    let exit = loop {
        if let Some(exit) = child.try_wait().expect("poll the relay") {
            break exit;
        }
        assert!(asked.elapsed() < Duration::from_secs(10), "the relay did not exit within 10 s of SIGTERM");
        std::thread::sleep(Duration::from_millis(50));
    };
    let took = asked.elapsed();
    assert!(exit.success(), "the relay exited with {exit}, not cleanly");
    assert!(took < Duration::from_secs(5), "the relay took {took:?} to stop");
}
