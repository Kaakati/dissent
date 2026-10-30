//! Test-only helpers shared by the unit tests.

use std::future::Future;
use std::time::Duration;

/// Longest any one test may run. A hang fails here instead of stalling the whole run. Multiplied by
/// `scale`, as `limit` gives it.
pub(crate) const TEST_LIMIT: Duration = Duration::from_secs(240);

/// How much longer than on a developer machine the tests may wait, from `DISSENT_TEST_SCALE` (default
/// 1, never below it). CI sets it for its small runners. Only the time a failing check takes grows: a
/// passing one returns as soon as it passes.
pub(crate) fn scale() -> f64 {
    static SCALE: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *SCALE.get_or_init(|| {
        std::env::var("DISSENT_TEST_SCALE")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|s| s.is_finite())
            .map_or(1.0, |s| s.max(1.0))
    })
}

/// `d` multiplied by `scale`.
pub(crate) fn scaled(d: Duration) -> Duration {
    d.mul_f64(scale())
}

/// `TEST_LIMIT`, scaled.
pub(crate) fn limit() -> Duration {
    scaled(TEST_LIMIT)
}

/// Runs a test body, failing it if it does not finish within `limit()`.
pub(crate) async fn bounded(body: impl Future<Output = ()>) {
    let limit = limit();
    tokio::time::timeout(limit, body)
        .await
        .unwrap_or_else(|_| panic!("test exceeded {limit:?}"));
}

/// Runs a test body on its own OS thread, with its own runtime, and fails if it does not finish within `limit()`.
/// `bounded` cannot catch a body that blocks its thread, because its timer runs on that same task. This waits on a
/// channel instead, so a stuck call fails the test in minutes rather than hanging the run.
pub(crate) fn on_own_thread<Fut: Future<Output = ()>>(body: impl FnOnce() -> Fut + Send + 'static) {
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("test-body".into())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .expect("test runtime")
                    .block_on(body())
            }));
            let _ = done.send(outcome);
        })
        .expect("spawn test body");
    let limit = limit();
    match finished.recv_timeout(limit) {
        Ok(Ok(())) => {}
        Ok(Err(panic)) => std::panic::resume_unwind(panic),
        Err(_) => panic!("test body did not finish within {limit:?}: a call inside it is blocked"),
    }
}
