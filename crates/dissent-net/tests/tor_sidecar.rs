//! The built-in Tor's supervisor against a fake arti: this test program itself. Run as `<exe> proxy -c <config>`, as
//! the supervisor runs arti, it acts as arti does: it reads the port file's path from the config, opens a SOCKS5 port
//! on loopback, writes it there, and carries tunnels to their target (here, each tunnel echoes). What else it does
//! comes from a `fake-mode` file next to the config: `obsolete` prints arti's obsolete-version line and exits.
//!
//! Without a harness, so `main` can be the fake. Only loopback is used.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use dissent_net::tor_sidecar::OBSOLETE_MARKER;
use dissent_net::{SidecarState, SocksBridge, TorSidecar};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const WAIT: Duration = Duration::from_secs(20);

type Test = std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>;
type Case = (&'static str, fn() -> Test);

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("proxy") {
        fake_arti(&args);
        return;
    }
    if args.get(1).map(String::as_str) == Some("fake-app") {
        fake_app(Path::new(&args[2]));
        return;
    }
    let runtime = tokio::runtime::Runtime::new().unwrap();
    #[allow(unused_mut)]
    let mut tests: Vec<Case> = vec![
        ("tunnels_go_through_the_port_it_reports", || Box::pin(tunnels_go_through_the_port_it_reports())),
        ("a_killed_tor_is_started_again_and_nothing_gets_out_meanwhile", || {
            Box::pin(a_killed_tor_is_started_again_and_nothing_gets_out_meanwhile())
        }),
        ("an_obsolete_version_stays_stopped", || Box::pin(an_obsolete_version_stays_stopped())),
        ("a_missing_program_fails_at_once", || Box::pin(a_missing_program_fails_at_once())),
        ("stop_kills_it", || Box::pin(stop_kills_it())),
    ];
    #[cfg(unix)]
    tests.push(("an_arti_a_crashed_app_left_running_is_killed_at_the_next_start", || {
        Box::pin(an_arti_a_crashed_app_left_running_is_killed_at_the_next_start())
    }));
    #[cfg(target_os = "linux")]
    tests.push(("arti_dies_with_an_app_that_is_killed", || Box::pin(arti_dies_with_an_app_that_is_killed())));
    #[cfg(target_os = "linux")]
    tests.push(("arti_outlives_the_thread_that_started_it", || Box::pin(arti_outlives_the_thread_that_started_it())));
    for &(name, test) in &tests {
        print!("test {name} ... ");
        runtime.block_on(test());
        println!("ok");
    }
    // The real arti, built by scripts/build-arti.sh, on the real Tor network: `-- --ignored` with DISSENT_ARTI set.
    let real = std::env::var_os("DISSENT_ARTI").filter(|_| args.iter().any(|a| a == "--ignored"));
    if let Some(arti) = &real {
        print!("test the_real_arti_reaches_the_tor_network ... ");
        runtime.block_on(the_real_arti_reaches_the_tor_network(PathBuf::from(arti)));
        println!("ok");
    }
    let ran = tests.len() + usize::from(real.is_some());
    println!("\ntest result: ok. {ran} passed; {} ignored", usize::from(real.is_none()));
}

async fn the_real_arti_reaches_the_tor_network(arti: PathBuf) {
    let sidecar = TorSidecar::start(arti, folder("real")).unwrap();
    let socks = running(&sidecar).await;
    let reached = dissent_net::socks_bridge::reachable(socks, "example.com", 80, Duration::from_secs(120)).await;
    assert!(reached.is_ok(), "{reached:?}");
    assert!(sidecar.dir().join("state").is_dir() && sidecar.dir().join("cache").is_dir(), "its state stays in its folder");
    sidecar.stop().await;
}

/// A fresh folder for one test.
fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dissent-tor-sidecar-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn start(name: &str, mode: &str) -> TorSidecar {
    let dir = folder(name);
    std::fs::write(dir.join("fake-mode"), mode).unwrap();
    TorSidecar::start(std::env::current_exe().unwrap(), dir).unwrap()
}

/// Waits until the sidecar's state passes `check`, and returns it.
async fn until(sidecar: &TorSidecar, what: &str, check: impl Fn(&SidecarState) -> bool) -> SidecarState {
    let mut states = sidecar.watch_state();
    let found = tokio::time::timeout(WAIT, states.wait_for(|s| check(s))).await;
    match found {
        Ok(Ok(state)) => state.clone(),
        _ => panic!("never {what}; last {:?}", sidecar.state()),
    }
}

async fn running(sidecar: &TorSidecar) -> SocketAddr {
    match until(sidecar, "running", |s| matches!(s, SidecarState::Running(_))).await {
        SidecarState::Running(socks) => socks,
        _ => unreachable!(),
    }
}

/// The status code the bridge answers a tunnel with, and for a 200 whether the tunnel carries bytes.
async fn tunnel(bridge: &SocksBridge) -> u16 {
    let url = bridge.url("relay");
    let addr = url.socket_addrs(|| None).unwrap()[0];
    let login = data_encoding::BASE64.encode(format!("{}:{}", url.username(), url.password().unwrap()).as_bytes());
    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!("CONNECT relay.example.org:443 HTTP/1.1\r\nProxy-Authorization: Basic {login}\r\n\r\n");
    client.write_all(request.as_bytes()).await.unwrap();
    let mut status = [0u8; 12];
    client.read_exact(&mut status).await.unwrap();
    let code: u16 = std::str::from_utf8(&status[9..12]).unwrap().parse().unwrap();
    if code == 200 {
        let mut rest = [0u8; 27];
        client.read_exact(&mut rest).await.unwrap();
        client.write_all(b"ping").await.unwrap();
        let mut back = [0u8; 4];
        client.read_exact(&mut back).await.unwrap();
        assert_eq!(&back, b"ping", "the tunnel carries the bytes");
    }
    code
}

async fn tunnels_go_through_the_port_it_reports() {
    let sidecar = start("tunnels", "serve");
    let bridge = SocksBridge::follow(sidecar.socks()).await.unwrap();
    let socks = running(&sidecar).await;
    assert!(socks.ip().is_loopback());
    assert_eq!(*sidecar.socks().borrow(), Some(socks));
    assert_eq!(tunnel(&bridge).await, 200);
    let config = std::fs::read_to_string(sidecar.dir().join("arti.toml")).unwrap();
    assert!(config.contains("127.0.0.1:auto"), "{config}");
    sidecar.stop().await;
}

async fn a_killed_tor_is_started_again_and_nothing_gets_out_meanwhile() {
    let sidecar = start("restart", "serve");
    let bridge = SocksBridge::follow(sidecar.socks()).await.unwrap();
    running(&sidecar).await;
    let pid = std::fs::read_to_string(sidecar.dir().join("fake.pid")).unwrap();
    kill(pid.trim());
    until(&sidecar, "restarting", |s| matches!(s, SidecarState::Restarting(_))).await;
    assert_eq!(*sidecar.socks().borrow(), None, "no port while it is down");
    assert_eq!(tunnel(&bridge).await, 502, "fail closed while it is down");
    running(&sidecar).await;
    assert_ne!(std::fs::read_to_string(sidecar.dir().join("fake.pid")).unwrap(), pid, "a new process");
    assert_eq!(tunnel(&bridge).await, 200, "the bridge follows it to its new port");
    sidecar.stop().await;
}

async fn an_obsolete_version_stays_stopped() {
    let sidecar = start("obsolete", "obsolete");
    let bridge = SocksBridge::follow(sidecar.socks()).await.unwrap();
    until(&sidecar, "obsolete", |s| *s == SidecarState::Obsolete).await;
    // Longer than the first pause before a restart.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(sidecar.state(), SidecarState::Obsolete, "not started again");
    assert_eq!(*sidecar.socks().borrow(), None);
    assert_eq!(tunnel(&bridge).await, 502, "fail closed");
}

async fn a_missing_program_fails_at_once() {
    let dir = folder("missing");
    let sidecar = TorSidecar::start(dir.join("no-such-arti"), dir).unwrap();
    until(&sidecar, "failed", |s| matches!(s, SidecarState::Failed(_))).await;
    assert_eq!(*sidecar.socks().borrow(), None);
}

async fn stop_kills_it() {
    let sidecar = start("stop", "serve");
    let socks = running(&sidecar).await;
    assert!(TcpStream::connect(socks).is_ok());
    let states = sidecar.watch_state();
    sidecar.stop().await;
    assert_eq!(*states.borrow(), SidecarState::Stopped);
    assert!(TcpStream::connect_timeout(&socks, Duration::from_secs(2)).is_err(), "its port closed with it");
}

/// A crashed app leaves its arti running: here a fake arti this test starts itself, on the folder's config, with its
/// pid in `arti.pid` as the supervisor writes it. The next start kills it before it starts its own.
#[cfg(unix)]
async fn an_arti_a_crashed_app_left_running_is_killed_at_the_next_start() {
    let first = start("left-over", "serve");
    running(&first).await;
    let dir = first.dir().to_path_buf();
    first.stop().await;
    let mut orphan = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["proxy", "-c"])
        .arg(dir.join("arti.toml"))
        .spawn()
        .unwrap();
    std::fs::write(dir.join("arti.pid"), orphan.id().to_string()).unwrap();
    // Its port file says it runs.
    tokio::time::timeout(WAIT, async {
        while std::fs::read_to_string(dir.join("fake.pid")).ok().as_deref() != Some(orphan.id().to_string().as_str()) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the left-over arti started");
    assert!(orphan.try_wait().unwrap().is_none());
    let next = TorSidecar::start(std::env::current_exe().unwrap(), dir.clone()).unwrap();
    running(&next).await;
    let status = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(status) = orphan.try_wait().unwrap() {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the left-over arti was killed");
    assert!(!status.success(), "killed, not exited: {status:?}");
    next.stop().await;
}

/// `fake-app <dir>`: an app that starts its built-in Tor in `dir` and runs until it is killed.
fn fake_app(dir: &Path) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let sidecar = TorSidecar::start(std::env::current_exe().unwrap(), dir.to_path_buf()).unwrap();
        running(&sidecar).await;
        std::future::pending::<()>().await;
    });
}

/// The app is killed outright, so nothing in it stops arti: the death signal set in arti does.
#[cfg(target_os = "linux")]
async fn arti_dies_with_an_app_that_is_killed() {
    let dir = folder("app-killed");
    std::fs::write(dir.join("fake-mode"), "serve").unwrap();
    let mut app = std::process::Command::new(std::env::current_exe().unwrap()).arg("fake-app").arg(&dir).spawn().unwrap();
    let pid_file = dir.join("fake.pid");
    let arti = tokio::time::timeout(WAIT, async {
        loop {
            if let Some(pid) = std::fs::read_to_string(&pid_file).ok().filter(|p| !p.is_empty()) {
                return pid;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the app started its arti");
    // Running, and not a zombie waiting for whoever took it over to reap it.
    let alive = |pid: &str| {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| !stat.rsplit_once(") ").is_some_and(|(_, rest)| rest.starts_with('Z')))
    };
    assert!(alive(&arti));
    app.kill().unwrap();
    app.wait().unwrap();
    let gone = tokio::time::timeout(Duration::from_secs(5), async {
        while alive(&arti) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(gone.is_ok(), "arti {arti} outlived its app");
}

/// Linux's death signal comes when the thread that started arti ends. Started from a thread that ends at once, as a
/// blocking-pool thread does when idle, arti must keep running.
#[cfg(target_os = "linux")]
async fn arti_outlives_the_thread_that_started_it() {
    let runtime = tokio::runtime::Handle::current();
    let sidecar = std::thread::spawn(move || {
        let _inside = runtime.enter();
        start("thread", "serve")
    })
    .join()
    .unwrap();
    running(&sidecar).await;
    let pid = std::fs::read_to_string(sidecar.dir().join("fake.pid")).unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(sidecar.state(), SidecarState::Running(sidecar.socks().borrow().unwrap()), "still running");
    assert!(Path::new(&format!("/proc/{pid}")).exists(), "arti {pid} was killed with the thread");
    sidecar.stop().await;
}

fn kill(pid: &str) {
    let status = if cfg!(windows) {
        std::process::Command::new("taskkill").args(["/F", "/PID", pid]).stdout(std::process::Stdio::null()).status()
    } else {
        std::process::Command::new("kill").args(["-9", pid]).status()
    };
    assert!(status.unwrap().success(), "could not kill the fake arti");
}

/// The fake arti: `proxy -c <config>`.
fn fake_arti(args: &[String]) {
    let config = Path::new(args.iter().skip_while(|a| *a != "-c").nth(1).expect("-c <config>"));
    let dir = config.parent().unwrap();
    let text = std::fs::read_to_string(config).unwrap();
    let port_info = text
        .lines()
        .find_map(|l| l.strip_prefix("port_info_file = { literal = \"")?.strip_suffix("\" }"))
        .map(|p| p.replace("\\\\", "\\").replace("\\\"", "\""))
        .expect("a literal port_info_file");
    let mode = std::fs::read_to_string(dir.join("fake-mode")).unwrap_or_default();
    std::fs::write(dir.join("fake.pid"), std::process::id().to_string()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::fs::write(&port_info, format!(r#"{{"ports":[{{"protocol":"socks","address":"inet:{addr}"}}]}}"#)).unwrap();
    if mode.trim() == "obsolete" {
        std::thread::sleep(Duration::from_millis(300));
        eprintln!("{OBSOLETE_MARKER}.\nError was:\nrequired protocols are missing");
        std::thread::sleep(Duration::from_millis(200));
        std::process::exit(1);
    }
    for client in listener.incoming().flatten() {
        std::thread::spawn(move || socks_echo(client));
    }
}

/// A SOCKS5 server that accepts any login and any target, and echoes what the tunnel carries.
fn socks_echo(mut s: TcpStream) -> std::io::Result<()> {
    let mut hello = [0u8; 2];
    s.read_exact(&mut hello)?;
    let mut methods = vec![0u8; usize::from(hello[1])];
    s.read_exact(&mut methods)?;
    s.write_all(&[5, 2])?;
    let mut ver_len = [0u8; 2];
    s.read_exact(&mut ver_len)?;
    let mut user = vec![0u8; usize::from(ver_len[1])];
    s.read_exact(&mut user)?;
    let mut plen = [0u8; 1];
    s.read_exact(&mut plen)?;
    let mut pass = vec![0u8; usize::from(plen[0])];
    s.read_exact(&mut pass)?;
    s.write_all(&[1, 0])?;
    let mut head = [0u8; 5];
    s.read_exact(&mut head)?;
    let mut rest = vec![0u8; usize::from(head[4]) + 2];
    s.read_exact(&mut rest)?;
    s.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0])?;
    let mut buf = [0u8; 1024];
    loop {
        let n = s.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        s.write_all(&buf[..n])?;
    }
}
