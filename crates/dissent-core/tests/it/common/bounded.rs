use std::future::Future;
use std::time::Duration;

/// Longest any one test may run. A hang fails here instead of stalling the whole run.
pub const TEST_LIMIT: Duration = Duration::from_secs(240);

/// Runs a test body, failing it if it does not finish within `TEST_LIMIT`.
pub async fn bounded(body: impl Future<Output = ()>) {
    tokio::time::timeout(TEST_LIMIT, body)
        .await
        .unwrap_or_else(|_| panic!("test exceeded {TEST_LIMIT:?}"));
}

/// Like `bounded`, for one long test that needs more than `TEST_LIMIT`, such as a seven-device call
/// set up on a slow runner.
pub async fn bounded_within(limit: Duration, body: impl Future<Output = ()>) {
    tokio::time::timeout(limit, body)
        .await
        .unwrap_or_else(|_| panic!("test exceeded {limit:?}"));
}
