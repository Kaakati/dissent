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
    HEAVY_ENGINES
        .scope(engines, async {
            let limit = scaled(TEST_LIMIT);
            tokio::time::timeout(limit, body)
                .await
                .unwrap_or_else(|_| panic!("test exceeded {limit:?}"));
        })
        .await;
}
