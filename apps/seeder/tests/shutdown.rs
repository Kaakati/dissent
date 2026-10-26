//! The seeder stops cleanly and promptly when it is sent SIGTERM, as a service manager sends it. It is
//! configured through environment variables, as a systemd unit configures it. Unix only: a Windows service
//! is stopped by the Service Control Manager, and that path is not run here.
#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Every line the child writes to stderr, read on a thread so the child never blocks on a full pipe.
fn stderr_lines(child: &mut std::process::Child) -> mpsc::Receiver<String> {
    let stderr = child.stderr.take().expect("stderr is piped");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    rx
}

/// Waits until a line containing `needle` arrives, and returns every line seen so far.
fn wait_for(lines: &mpsc::Receiver<String>, needle: &str, limit: Duration) -> Vec<String> {
    let mut seen = Vec::new();
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        match lines.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                let done = line.contains(needle);
                seen.push(line);
                if done {
                    return seen;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    panic!("no line containing {needle:?} within {limit:?}; saw {seen:?}");
}

#[test]
fn the_seeder_exits_cleanly_on_sigterm_within_seconds() {
    let dir = std::env::temp_dir().join(format!("dissent-seeder-shutdown-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let passphrase = dir.join("passphrase.txt");
    std::fs::write(&passphrase, "shutdown test passphrase\n").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_dissent-seeder"))
        .env("DISSENT_SEEDER_DATA_DIR", dir.join("data"))
        .env("DISSENT_SEEDER_LOOPBACK", "true")
        .env("DISSENT_SEEDER_PASSPHRASE_FILE", &passphrase)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start the seeder");
    let lines = stderr_lines(&mut child);

    // The first status line means the engine is running and the run loop is listening for a stop.
    wait_for(&lines, "network status", Duration::from_secs(60));
    // The loop polls for a stop every second, so the signal handler is in place a second later.
    std::thread::sleep(Duration::from_millis(1500));

    let asked = Instant::now();
    let status = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("send SIGTERM");
    assert!(status.success(), "kill -TERM failed");

    let exit = loop {
        if let Some(exit) = child.try_wait().expect("poll the seeder") {
            break exit;
        }
        assert!(asked.elapsed() < Duration::from_secs(10), "the seeder did not exit within 10 s of SIGTERM");
        std::thread::sleep(Duration::from_millis(50));
    };
    let took = asked.elapsed();
    assert!(exit.success(), "the seeder exited with {exit}, not cleanly");
    assert!(took < Duration::from_secs(5), "the seeder took {took:?} to stop");
    let rest = wait_for(&lines, "shutdown requested", Duration::from_secs(5));
    assert!(rest.iter().any(|l| l.contains("shutdown requested")));
    let _ = std::fs::remove_dir_all(&dir);
}
