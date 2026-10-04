//! Small timing helpers every journey uses.

use std::future::Future;
use std::time::Duration;

/// Run one journey with a hard deadline, so a hang fails the test with its name
/// instead of wedging the whole run until CI's own timeout. The containers a
/// journey started are still removed: they drop when the future does.
pub async fn journey<F, T>(name: &str, deadline: Duration, body: F) -> T
where
    F: Future<Output = T>,
{
    match tokio::time::timeout(deadline, body).await {
        Ok(value) => value,
        Err(_) => panic!("journey `{name}` did not finish within {deadline:?}"),
    }
}

/// Ask `probe` until it returns `Some`, or panic naming `what` when `within`
/// runs out. Every wait in a journey is a condition polled, never a bare sleep
/// sized to a guess.
pub async fn eventually<T, F, Fut>(what: &str, within: Duration, mut probe: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let deadline = tokio::time::Instant::now() + within;
    loop {
        if let Some(value) = probe().await {
            return value;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("timed out after {within:?} waiting for {what}");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// A short random suffix for container, network and agent names, so journeys
/// running side by side never collide.
pub fn suffix() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..10].to_string()
}

/// Quote `text` for a POSIX shell, single quotes and all.
pub fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}
