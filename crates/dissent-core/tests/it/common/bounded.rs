use std::future::Future;
use std::time::Duration;

/// Longest any one test may run. A hang fails here instead of stalling the whole run. Multiplied by
/// `scale`.
pub const TEST_LIMIT: Duration = Duration::from_secs(240);

/// How much longer than on a developer machine the tests may wait, from `DISSENT_TEST_SCALE` (default
/// 1, never below it). CI sets it for its small runners. It stretches `bounded`, `heavy` and the
/// `eventually` family. A passing check returns as soon as it passes, so only a failing one waits
/// longer.
pub fn scale() -> f64 {
    static SCALE: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *SCALE.get_or_init(|| {
        std::env::var("DISSENT_TEST_SCALE")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|s| s.is_finite())
            .map_or(1.0, |s| s.max(1.0))
    })
}

tokio::task_local! {
    /// The engines the running `heavy` test starts, so its waits can grow with them.
    static HEAVY_ENGINES: usize;
}

/// Engines a test may start before its waits grow. A heavy test's waits grow in proportion above it,
/// since every engine costs CPU on the same runner.
const ENGINES_AT_SCALE_ONE: usize = 5;

/// `d` stretched by `scale`, and inside a `heavy` test by its engine count over
/// `ENGINES_AT_SCALE_ONE`.
pub fn scaled(d: Duration) -> Duration {
    let engines = HEAVY_ENGINES.try_with(|n| *n).unwrap_or(0);
    let load = (engines as f64 / ENGINES_AT_SCALE_ONE as f64).max(1.0);
    d.mul_f64(scale() * load)
}

/// Every networked test takes this lock: `bounded` shared, `heavy` alone. The lock is fair, so a
/// waiting heavy test holds back new tests until the running ones finish, then runs by itself. A
/// heavy test that ran beside others on a four-core runner timed out in its waits.
fn run_lock() -> &'static tokio::sync::RwLock<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::RwLock<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::RwLock::new(()))
}

/// Runs a test body, failing it if it does not finish within `TEST_LIMIT` (scaled). The time spent
/// waiting for a heavy test to finish does not count.
pub async fn bounded(body: impl Future<Output = ()>) {
    bounded_within(TEST_LIMIT, body).await;
}

/// Like `bounded`, for a test that needs longer than `TEST_LIMIT`.
pub async fn bounded_within(limit: Duration, body: impl Future<Output = ()>) {
    let _shared = run_lock().read().await;
    let _watch = Watchdog::start();
    let limit = scaled(limit);
    tokio::time::timeout(limit, body)
        .await
        .unwrap_or_else(|_| panic!("test exceeded {limit:?}"));
}

/// Runs a test that starts `engines` engines on this machine, alone: no other `bounded` or `heavy`
/// test runs beside it. Its limit, and every `eventually` wait inside it, grow with `engines` (see
/// `scaled`). The time spent waiting for the running tests to finish does not count.
pub async fn heavy(engines: usize, body: impl Future<Output = ()>) {
    let _alone = run_lock().write().await;
    let _watch = Watchdog::start();
    HEAVY_ENGINES
        .scope(engines, async {
            let limit = scaled(TEST_LIMIT);
            tokio::time::timeout(limit, body)
                .await
                .unwrap_or_else(|_| panic!("test exceeded {limit:?}"));
        })
        .await;
}

/// The engines each running test started, by test name, for the watchdog. Held weakly, so a test that stops an
/// engine and starts it again on the same port finds the port free. Listed until the test ends.
type Watched = Vec<(String, String, dissent_core::test_support::WeakEngine)>;

fn engines() -> &'static std::sync::Mutex<Watched> {
    static ENGINES: std::sync::OnceLock<std::sync::Mutex<Watched>> = std::sync::OnceLock::new();
    ENGINES.get_or_init(Default::default)
}

fn test_name() -> String {
    std::thread::current().name().unwrap_or("?").to_string()
}

/// Lists an engine under the running test, for the watchdog.
pub fn watch_engine(name: &str, engine: &dissent_core::Engine) {
    engines().lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((
        test_name(),
        name.to_string(),
        dissent_core::test_support::WeakEngine::of(engine),
    ));
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// A call loop's breadcrumb that has sat on one step for over `STALL`. Waiting that long for a 20 ms tick means the
/// runtime did not poll the loop.
fn stuck(crumb: &str) -> bool {
    crumb
            .split(" for ")
            .nth(1)
            .and_then(|rest| rest.split(' ').next())
            .and_then(|ms| ms.parse::<u64>().ok())
            .is_some_and(|ms| ms > STALL)
}

const STALL: u64 = 5_000;

/// Catches a stalled runtime or call loop in the act. A task on the test's runtime beats every 250 ms. An OS thread
/// checks every second and prints when the beat, or an engine's call-loop step, is older than `STALL`: the test, how
/// long, and every engine's call-loop breadcrumbs. Each stall is reported once, and again when it ends.
struct Watchdog {
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    beat: tokio::task::JoinHandle<()>,
    test: String,
}

impl Watchdog {
    fn start() -> Self {
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        use std::sync::Arc;
        let test = test_name();
        let done = Arc::new(AtomicBool::new(false));
        let last = Arc::new(AtomicU64::new(now_ms()));
        let beat = {
            let last = last.clone();
            tokio::spawn(async move {
                loop {
                    last.store(now_ms(), Ordering::Relaxed);
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            })
        };
        let (watch_done, watch_test) = (done.clone(), test.clone());
        std::thread::spawn(move || {
            let mut stalled = false;
            let mut stuck_loops = std::collections::BTreeSet::new();
            while !watch_done.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_secs(1));
                let ours: Vec<(String, dissent_core::Engine)> = engines()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .iter()
                    .filter(|(t, _, _)| *t == watch_test)
                    .filter_map(|(_, n, e)| Some((n.clone(), e.upgrade()?)))
                    .collect();
                let crumbs: Vec<(String, String)> =
                    ours.iter().map(|(n, e)| (n.clone(), dissent_core::test_support::call_breadcrumbs(e))).collect();
                let age = now_ms().saturating_sub(last.load(Ordering::Relaxed));
                if age > STALL && !stalled {
                    stalled = true;
                    eprintln!("WATCHDOG {watch_test}: runtime heartbeat stalled {age} ms; engines: {crumbs:?}");
                } else if age <= STALL && stalled {
                    stalled = false;
                    eprintln!("WATCHDOG {watch_test}: runtime heartbeat resumed");
                }
                for (name, crumb) in &crumbs {
                    if stuck(crumb) {
                        if stuck_loops.insert(name.clone()) {
                            eprintln!("WATCHDOG {watch_test}: call loop of {name} stuck: {crumb}; all: {crumbs:?}");
                        }
                    } else if stuck_loops.remove(name) {
                        eprintln!("WATCHDOG {watch_test}: call loop of {name} moving again: {crumb}");
                    }
                }
            }
        });
        Watchdog { done, beat, test }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.done.store(true, std::sync::atomic::Ordering::Relaxed);
        self.beat.abort();
        let test = &self.test;
        engines().lock().unwrap_or_else(std::sync::PoisonError::into_inner).retain(|(t, _, _)| t != test);
    }
}
