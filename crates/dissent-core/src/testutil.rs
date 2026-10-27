//! Test-only helpers shared by the unit tests.

use std::future::Future;
use std::time::Duration;

/// Longest any one test may run. A hang fails here instead of stalling the whole run.
pub(crate) const TEST_LIMIT: Duration = Duration::from_secs(240);

/// Runs a test body, failing it if it does not finish within `TEST_LIMIT`.
pub(crate) async fn bounded(body: impl Future<Output = ()>) {
    tokio::time::timeout(TEST_LIMIT, body)
        .await
        .unwrap_or_else(|_| panic!("test exceeded {TEST_LIMIT:?}"));
}

/// Runs a test body on its own OS thread, with its own runtime, and fails if it does not finish within `TEST_LIMIT`.
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
    match finished.recv_timeout(TEST_LIMIT) {
        Ok(Ok(())) => {}
        Ok(Err(panic)) => std::panic::resume_unwind(panic),
        Err(_) => panic!("test body did not finish within {TEST_LIMIT:?}: a call inside it is blocked"),
    }
}
