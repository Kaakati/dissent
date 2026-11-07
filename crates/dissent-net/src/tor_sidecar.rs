//! "Hide my IP through Tor" with the Tor built into the app: arti, run as a program of its own next to the app (a
//! sidecar), not linked in. arti ends its own process when the Tor network stops accepting its version (it calls
//! `process::exit` with no way to turn that off), and in a program of its own that ends Tor, not the app.
//!
//! [`TorSidecar`] starts `arti proxy` with a config this module writes into the profile's `tor` folder: a SOCKS port
//! on loopback that the system picks (`127.0.0.1:auto`), which arti writes to `port_info.json`; no DNS port; no HTTP
//! CONNECT on the SOCKS port; its state and directory cache in the same folder. The port it reports goes out on a
//! watch channel, which the bridge (`SocksBridge::follow`) reads for each tunnel, and which holds `None` whenever arti
//! is not running, so nothing gets out then.
//!
//! When arti exits it is started again after a pause that doubles up to a minute, except when it said its version is
//! obsolete ([`OBSOLETE_MARKER`] on its stderr): then it stays stopped, and only an app update brings Tor back. The
//! app kills it when it quits or switches profile. When the app dies instead, arti goes with it: on Windows through a
//! job object, on Linux through a death signal set in the child. On Linux and macOS each start also writes arti's pid
//! to `arti.pid`, and the next start kills an arti left running under that pid, but only when that process runs this
//! same program, never on the pid alone.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;

use crate::error::{NetError, Result};

/// The arti release the app is built with (`scripts/build-arti.sh` builds this one). [`OBSOLETE_MARKER`] was checked
/// against its source; a test keeps the two in step.
pub const ARTI_VERSION: &str = "2.8.0";

/// What arti 2.8.0 prints to stderr before it exits because the Tor network requires a protocol it lacks
/// (arti-client 0.48.0, `client.rs`, the `enforce_protocol_recommendations` callback). The build script fails when
/// the arti source it builds no longer contains it.
pub const OBSOLETE_MARKER: &str = "Shutting down because of unsupported software version";

/// The config file, the port file, and arti's own folders, inside the folder given to [`TorSidecar::start`].
const CONFIG_FILE: &str = "arti.toml";
const PORT_INFO_FILE: &str = "port_info.json";
const STATE_DIR: &str = "state";
const CACHE_DIR: &str = "cache";
#[cfg(unix)]
const PID_FILE: &str = "arti.pid";

/// How long arti may take to open its SOCKS port. It opens it before it bootstraps, within a second or two.
const PORT_WAIT: Duration = Duration::from_secs(30);
/// How often the port file is looked for while arti starts.
const PORT_POLL: Duration = Duration::from_millis(100);
/// The first pause before arti is started again, doubled after each exit up to `BACKOFF_MAX`.
const BACKOFF_FIRST: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// A run this long counts as healthy: the next exit pauses for `BACKOFF_FIRST` again.
const STABLE_RUN: Duration = Duration::from_secs(60);

/// Where the built-in Tor is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SidecarState {
    /// Started; its SOCKS port is not open yet.
    Starting,
    /// Its SOCKS port is open here. It may still be building its first circuits.
    Running(SocketAddr),
    /// It stopped, and is started again after a pause. Why it stopped.
    Restarting(String),
    /// Stopped for good: the Tor network no longer accepts this arti version. Only an app update brings Tor back.
    Obsolete,
    /// It could not be started at all, for example because the program is missing. Why.
    Failed(String),
    /// Stopped by the app.
    Stopped,
}

/// The running built-in Tor and its supervisor. Dropping it kills arti; [`TorSidecar::stop`] also waits for it.
#[derive(Debug)]
pub struct TorSidecar {
    dir: PathBuf,
    state: watch::Receiver<SidecarState>,
    socks: watch::Receiver<Option<SocketAddr>>,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl TorSidecar {
    /// Writes arti's config into `dir` (created, the user's only on Unix) and starts `program` on it. Returns at once:
    /// the SOCKS port comes later on [`TorSidecar::socks`].
    pub fn start(program: PathBuf, dir: PathBuf) -> Result<TorSidecar> {
        private_dir(&dir)?;
        let config = dir.join(CONFIG_FILE);
        std::fs::write(&config, config_text(&dir)).map_err(|e| NetError::Tor(format!("could not write {}: {e}", config.display())))?;
        let (state_tx, state) = watch::channel(SidecarState::Starting);
        let (socks_tx, socks) = watch::channel(None);
        let (stop, stop_rx) = oneshot::channel();
        let run = Run {
            program,
            config,
            port_info: dir.join(PORT_INFO_FILE),
            #[cfg(unix)]
            pid_file: dir.join(PID_FILE),
            state: state_tx,
            socks: socks_tx,
        };
        let task = tokio::spawn(run.supervise(stop_rx));
        Ok(TorSidecar { dir, state, socks, stop: Some(stop), task })
    }

    /// The folder it runs in.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Its SOCKS address while it runs, `None` otherwise: for `SocksBridge::follow`.
    pub fn socks(&self) -> watch::Receiver<Option<SocketAddr>> {
        self.socks.clone()
    }

    /// Where it is now.
    pub fn state(&self) -> SidecarState {
        self.state.borrow().clone()
    }

    /// Every change of [`TorSidecar::state`].
    pub fn watch_state(&self) -> watch::Receiver<SidecarState> {
        self.state.clone()
    }

    /// Kills arti and waits until it has exited.
    pub async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let _ = (&mut self.task).await;
    }
}

impl Drop for TorSidecar {
    fn drop(&mut self) {
        // The supervisor owns the child, which is killed when dropped.
        self.task.abort();
    }
}

/// The config arti runs on, all of it inside `dir`. Paths are literal, so nothing in them is expanded.
fn config_text(dir: &Path) -> String {
    let path = |name: &str| format!("{{ literal = {} }}", toml_string(&dir.join(name).to_string_lossy()));
    format!(
        "# Written by Dissent each time it starts its built-in Tor. Changes here are lost.\n\
         [proxy]\n\
         socks_listen = \"127.0.0.1:auto\"\n\
         dns_listen = false\n\
         enable_http_connect = false\n\
         \n\
         [storage]\n\
         state_dir = {}\n\
         cache_dir = {}\n\
         port_info_file = {}\n",
        path(STATE_DIR),
        path(CACHE_DIR),
        path(PORT_INFO_FILE),
    )
}

/// `s` as a TOML basic string.
fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The loopback SOCKS address in arti's `port_info.json`. Its format is arti's (`crates/arti/src/proxy/port_info.rs`):
/// `{"ports":[{"protocol":"socks","address":"inet:127.0.0.1:41234"}]}`, where readers skip what they don't know.
fn socks_in_port_info(json: &str) -> Option<SocketAddr> {
    #[derive(serde::Deserialize)]
    struct PortInfo {
        ports: Vec<Port>,
    }
    #[derive(serde::Deserialize)]
    struct Port {
        protocol: String,
        address: Option<String>,
    }
    let info: PortInfo = serde_json::from_str(json).ok()?;
    info.ports
        .into_iter()
        .filter(|p| p.protocol == "socks")
        .filter_map(|p| p.address?.strip_prefix("inet:")?.parse::<SocketAddr>().ok())
        .find(|a| a.ip().is_loopback())
}

/// Creates `dir`, on Unix as the user's only (arti refuses a state folder that others can read).
fn private_dir(dir: &Path) -> Result<()> {
    let failed = |e: std::io::Error| NetError::Tor(format!("could not create {}: {e}", dir.display()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(failed)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(failed)?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir).map_err(failed)?;
    Ok(())
}

/// How one run of arti ended.
enum Ended {
    /// The app stopped it.
    Stopped,
    /// It said its version is obsolete, and exited.
    Obsolete,
    /// It exited or never opened its port. Why.
    Exited(String),
    /// The program could not be started. Why.
    NotStarted(String),
}

struct Run {
    program: PathBuf,
    config: PathBuf,
    port_info: PathBuf,
    #[cfg(unix)]
    pid_file: PathBuf,
    state: watch::Sender<SidecarState>,
    socks: watch::Sender<Option<SocketAddr>>,
}

impl Run {
    async fn supervise(self, mut stop: oneshot::Receiver<()>) {
        // Kills arti when the app dies, which nothing else here sees. Held for the supervisor's life.
        #[cfg(windows)]
        let job = job::Job::new();
        let mut backoff = BACKOFF_FIRST;
        loop {
            let started = Instant::now();
            #[cfg(windows)]
            let ended = self.run_once(&mut stop, job.as_ref()).await;
            #[cfg(not(windows))]
            let ended = self.run_once(&mut stop).await;
            self.socks.send_replace(None);
            match ended {
                Ended::Stopped => {
                    self.state.send_replace(SidecarState::Stopped);
                    return;
                }
                Ended::Obsolete => {
                    tracing::warn!(version = ARTI_VERSION, "the built-in Tor stopped: the Tor network no longer accepts this version");
                    self.state.send_replace(SidecarState::Obsolete);
                    return;
                }
                Ended::NotStarted(why) => {
                    tracing::warn!(error = %why, "the built-in Tor could not start");
                    self.state.send_replace(SidecarState::Failed(why));
                    return;
                }
                Ended::Exited(why) => {
                    if started.elapsed() >= STABLE_RUN {
                        backoff = BACKOFF_FIRST;
                    }
                    tracing::warn!(error = %why, wait = ?backoff, "the built-in Tor stopped; starting it again");
                    self.state.send_replace(SidecarState::Restarting(why));
                    tokio::select! {
                        _ = tokio::time::sleep(backoff) => {}
                        _ = &mut stop => {
                            self.state.send_replace(SidecarState::Stopped);
                            return;
                        }
                    }
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                }
            }
        }
    }

    async fn run_once(&self, stop: &mut oneshot::Receiver<()>, #[cfg(windows)] job: Option<&job::Job>) -> Ended {
        // A file left by an earlier run names a port nothing listens on any more.
        let _ = std::fs::remove_file(&self.port_info);
        // An arti a crashed app left running would keep its SOCKS port open, with nothing to stop it.
        #[cfg(unix)]
        left_over::kill(&self.pid_file, &self.program).await;
        let mut command = Command::new(&self.program);
        command
            .arg("proxy")
            .arg("-c")
            .arg(&self.config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            // No console window for it.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        #[cfg(target_os = "linux")]
        {
            let app = std::process::id() as libc::pid_t;
            // SAFETY: between fork and exec this calls only prctl, getppid and _exit, all async-signal-safe, and
            // touches no memory but its own copy of `app`.
            unsafe {
                command.pre_exec(move || {
                    // SIGKILL when the thread that started it ends: a tokio worker, which lives as long as the app.
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    // The app died before the line above, so the signal will never come.
                    if libc::getppid() != app {
                        libc::_exit(1);
                    }
                    Ok(())
                });
            }
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(e) => return Ended::NotStarted(format!("could not start {}: {e}", self.program.display())),
        };
        #[cfg(unix)]
        if let Some(pid) = child.id() {
            if let Err(e) = std::fs::write(&self.pid_file, pid.to_string()) {
                tracing::debug!(error = %e, "could not note the built-in Tor's pid");
            }
        }
        #[cfg(windows)]
        if let Some(job) = job {
            job.assign(&child);
        }
        self.state.send_replace(SidecarState::Starting);
        let obsolete = Arc::new(AtomicBool::new(false));
        let stderr = child.stderr.take().map(|err| tokio::spawn(read_stderr(err, obsolete.clone())));

        let deadline = Instant::now() + PORT_WAIT;
        let socks = loop {
            tokio::select! {
                status = child.wait() => return exited(status, stderr, &obsolete).await,
                _ = &mut *stop => return kill(child).await,
                _ = tokio::time::sleep(PORT_POLL) => {
                    if let Some(socks) = std::fs::read_to_string(&self.port_info).ok().as_deref().and_then(socks_in_port_info) {
                        break socks;
                    }
                    if Instant::now() >= deadline {
                        kill(child).await;
                        return Ended::Exited("its SOCKS port did not open in time".into());
                    }
                }
            }
        };
        tracing::info!(%socks, version = ARTI_VERSION, "the built-in Tor is running");
        self.socks.send_replace(Some(socks));
        self.state.send_replace(SidecarState::Running(socks));
        tokio::select! {
            status = child.wait() => exited(status, stderr, &obsolete).await,
            _ = &mut *stop => kill(child).await,
        }
    }
}

async fn kill(mut child: Child) -> Ended {
    let _ = child.kill().await;
    Ended::Stopped
}

/// How a run ended once arti exited with `status`. Its stderr is read to the end first: the obsolete-version line
/// comes seconds before the exit, but is only counted once it has been seen.
async fn exited(status: std::io::Result<std::process::ExitStatus>, stderr: Option<JoinHandle<()>>, obsolete: &AtomicBool) -> Ended {
    if let Some(reader) = stderr {
        let _ = tokio::time::timeout(Duration::from_secs(2), reader).await;
    }
    if obsolete.load(Ordering::Relaxed) {
        return Ended::Obsolete;
    }
    match status {
        Ok(status) => Ended::Exited(format!("it exited ({status})")),
        Err(e) => Ended::Exited(format!("it could not be waited for: {e}")),
    }
}

/// Passes arti's log to the app's, and notes when it says its version is obsolete.
async fn read_stderr(stderr: tokio::process::ChildStderr, obsolete: Arc<AtomicBool>) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.contains(OBSOLETE_MARKER) {
            obsolete.store(true, Ordering::Relaxed);
            tracing::warn!(target: "arti", "{line}");
        } else {
            tracing::debug!(target: "arti", "{line}");
        }
    }
}

/// An arti that a crashed app left running, found by the pid the last start wrote.
#[cfg(unix)]
mod left_over {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    /// The pid in `pid_file`, when the process with that pid runs `program`. A pid alone may have been reused by any
    /// other process since.
    pub(super) fn ours(pid_file: &Path, program: &Path) -> Option<libc::pid_t> {
        let pid: libc::pid_t = std::fs::read_to_string(pid_file).ok()?.trim().parse().ok()?;
        if pid <= 0 || pid as u32 == std::process::id() {
            return None;
        }
        let running = exe_of(pid)?.canonicalize().ok()?;
        (running == program.canonicalize().ok()?).then_some(pid)
    }

    /// Kills the arti `ours` finds, and waits up to two seconds for it to go.
    pub(super) async fn kill(pid_file: &Path, program: &Path) {
        let Some(pid) = ours(pid_file, program) else { return };
        tracing::warn!(pid, "stopping a built-in Tor left running by an earlier start");
        // SAFETY: plain syscalls on a pid just checked to run our arti.
        unsafe { libc::kill(pid, libc::SIGKILL) };
        for _ in 0..20 {
            // SAFETY: signal 0 only checks that the process exists.
            if unsafe { libc::kill(pid, 0) } != 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// The program a process runs. `None` when it is gone, or a zombie, or not ours to look at.
    #[cfg(target_os = "linux")]
    fn exe_of(pid: libc::pid_t) -> Option<PathBuf> {
        std::fs::read_link(format!("/proc/{pid}/exe")).ok()
    }

    #[cfg(target_os = "macos")]
    fn exe_of(pid: libc::pid_t) -> Option<PathBuf> {
        use std::os::unix::ffi::OsStringExt;
        let mut path = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: the buffer is as long as the size passed, and the call writes at most that much.
        let len = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
        if len <= 0 {
            return None;
        }
        path.truncate(len as usize);
        Some(PathBuf::from(std::ffi::OsString::from_vec(path)))
    }

    /// Elsewhere there is no way here to tell what a pid runs, so nothing is killed.
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn exe_of(_pid: libc::pid_t) -> Option<PathBuf> {
        None
    }
}

/// A job object that kills the processes in it when its last handle closes: when the app exits in any way, since
/// Windows closes the handles of a process that ends.
#[cfg(windows)]
mod job {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    pub struct Job(HANDLE);

    // SAFETY: a job handle may be used and closed from any thread.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub fn new() -> Option<Job> {
            // SAFETY: an unnamed job with default security; the limit struct is plain data, zeroed then filled, and
            // the size passed is its own.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return None;
                }
                let job = Job(handle);
                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let set = SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    std::ptr::from_ref(&limits).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                (set != 0).then_some(job)
            }
        }

        /// Puts `child` in the job. A child that can't be put in it still runs; only the kill on a crash is lost.
        pub fn assign(&self, child: &tokio::process::Child) {
            let Some(process) = child.raw_handle() else { return };
            // SAFETY: both handles are open: the job's is ours, and the child's lives as long as `child`.
            if unsafe { AssignProcessToJobObject(self.0, process as HANDLE) } == 0 {
                tracing::debug!("the built-in Tor could not be tied to the app's lifetime");
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle is ours and closed only here.
            unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_config_opens_only_a_loopback_socks_port_and_keeps_everything_in_its_folder() {
        let dir = Path::new(r#"C:\Users\a "b"\Dissent\profile\tor"#);
        let text = config_text(dir);
        assert!(text.contains("socks_listen = \"127.0.0.1:auto\""), "{text}");
        assert!(text.contains("dns_listen = false"));
        assert!(text.contains("enable_http_connect = false"));
        let state = format!("state_dir = {{ literal = {} }}", toml_string(&dir.join("state").to_string_lossy()));
        assert!(text.contains(&state), "{text}");
        assert!(text.contains(r#"\\Users\\a \"b\"\\"#), "backslashes and quotes escaped: {text}");
    }

    #[test]
    fn the_socks_port_is_read_from_arti_s_port_file() {
        let json = r#"{"ports":[{"protocol":"dns_udp","address":"inet:127.0.0.1:53"},{"protocol":"socks","address":"inet:127.0.0.1:41234"}]}"#;
        assert_eq!(socks_in_port_info(json), Some("127.0.0.1:41234".parse().unwrap()));
        let v6 = r#"{"ports":[{"protocol":"socks","address":"inet:[::1]:9150","extra":1}],"later":true}"#;
        assert_eq!(socks_in_port_info(v6), Some("[::1]:9150".parse().unwrap()), "unknown keys are skipped");
        assert_eq!(socks_in_port_info(r#"{"ports":[{"protocol":"socks","address":"inet:0.0.0.0:9150"}]}"#), None, "only loopback");
        assert_eq!(socks_in_port_info(r#"{"ports":[{"protocol":"socks","address":"unix:/run/arti.sock"}]}"#), None);
        assert_eq!(socks_in_port_info(r#"{"ports":[{"protocol":"socks","address":null}]}"#), None);
        assert_eq!(socks_in_port_info(r#"{"ports":[]}"#), None);
        assert_eq!(socks_in_port_info("{\"ports\":[{\"proto"), None, "a file half written");
    }

    /// A child of this test stands in for an arti left running: it runs the program named, or another one.
    #[cfg(unix)]
    #[test]
    fn a_left_over_pid_is_ours_only_when_it_runs_our_program() {
        let dir = std::env::temp_dir().join(format!("dissent-left-over-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pid_file = dir.join(PID_FILE);
        let sleep = PathBuf::from(if Path::new("/bin/sleep").exists() { "/bin/sleep" } else { "/usr/bin/sleep" });
        let mut child = std::process::Command::new(&sleep).arg("30").spawn().unwrap();
        std::fs::write(&pid_file, child.id().to_string()).unwrap();
        assert_eq!(left_over::ours(&pid_file, &sleep), Some(child.id() as libc::pid_t), "it runs the program");
        let elsewhere = dir.join("arti");
        std::fs::write(&elsewhere, b"").unwrap();
        assert_eq!(left_over::ours(&pid_file, &elsewhere), None, "a pid that runs another program is never ours");
        assert_eq!(left_over::ours(&pid_file, &dir.join("missing")), None);
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(left_over::ours(&pid_file, &sleep), None, "a pid that is gone");
        std::fs::write(&pid_file, std::process::id().to_string()).unwrap();
        assert_eq!(left_over::ours(&pid_file, &std::env::current_exe().unwrap()), None, "never the app itself");
        for junk in ["", "0", "-1", "abc"] {
            std::fs::write(&pid_file, junk).unwrap();
            assert_eq!(left_over::ours(&pid_file, &sleep), None, "{junk:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The version and marker here are the ones the build script builds and checks.
    #[test]
    fn the_build_script_builds_the_arti_this_code_was_checked_against() {
        let script = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/build-arti.sh")).unwrap();
        let value = |name: &str| {
            script
                .lines()
                .find_map(|l| l.strip_prefix(&format!("{name}=")))
                .map(|v| v.trim_matches('"').to_string())
                .unwrap_or_else(|| panic!("{name} is not set in build-arti.sh"))
        };
        assert_eq!(value("ARTI_VERSION"), ARTI_VERSION, "a new arti: check OBSOLETE_MARKER against its source");
        assert_eq!(value("ARTI_MARKER"), OBSOLETE_MARKER);
    }
}
