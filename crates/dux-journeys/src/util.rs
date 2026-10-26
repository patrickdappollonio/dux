//! Timing helpers every journey uses, and the record of what every container a
//! journey started has printed.

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::FutureExt as _;
use testcontainers::core::logs::LogFrame;
use testcontainers::core::logs::consumer::LogConsumer;

/// Everything one container has printed, shared between the container's log
/// stream and the journey that may need to print it.
#[derive(Clone, Debug, Default)]
pub(crate) struct LogBuffer(Arc<Mutex<String>>);

impl LogBuffer {
    /// Everything printed so far.
    pub(crate) fn text(&self) -> String {
        self.0.lock().expect("log buffer").clone()
    }
}

impl LogConsumer for LogBuffer {
    fn accept<'a>(&'a self, record: &'a LogFrame) -> futures_util::future::BoxFuture<'a, ()> {
        async move {
            let text = String::from_utf8_lossy(record.bytes());
            self.0.lock().expect("log buffer").push_str(&text);
        }
        .boxed()
    }
}

type Registry = Arc<Mutex<Vec<(String, LogBuffer)>>>;

tokio::task_local! {
    static JOURNEY_LOGS: Registry;
}

/// A fresh log buffer for container `name`, recorded against the running
/// journey so a failure or a timeout prints it.
pub(crate) fn container_logs(name: &str) -> LogBuffer {
    let buffer = LogBuffer::default();
    let _ = JOURNEY_LOGS.try_with(|registry| {
        registry
            .lock()
            .expect("journey log registry")
            .push((name.to_string(), buffer.clone()));
    });
    buffer
}

/// Print a progress line on stderr, through the test harness's capture, so it
/// is filed under the journey that printed it.
#[expect(
    clippy::print_stderr,
    reason = "only eprintln! reaches the test harness's output capture, which files a progress \
              line under the journey that printed it instead of interleaving it with the others"
)]
pub(crate) fn say(text: &str) {
    eprintln!("{text}");
}

/// Print every recorded container's output, through the test harness's capture so a failing
/// journey shows it under its own name.
#[expect(
    clippy::print_stderr,
    reason = "only eprintln! reaches the test harness's output capture, which files these logs \
              under the failing journey instead of interleaving them with the others"
)]
fn print_logs(registry: &Registry) {
    for (name, buffer) in registry.lock().expect("journey log registry").iter() {
        eprintln!(
            "\n----- everything {name} printed -----\n{}\n----- end of {name} -----\n",
            buffer.text()
        );
    }
}

/// Run one journey: the one-time setup first (an image build has its own,
/// generous deadline and never counts against a journey's), then the body with
/// a hard deadline, so a hang fails the test with its name instead of wedging
/// the run. When the body panics or times out, everything every container it
/// started printed (dux's output and dux.log included) is printed first. Its
/// containers are removed either way: they drop with the body.
///
/// # Panics
///
/// When the one-time setup fails, when `body` panics (its panic is resumed once the logs are
/// printed), or when it does not finish within `deadline`.
pub async fn journey<F, T>(name: &str, deadline: Duration, body: F) -> T
where
    F: Future<Output = T>,
{
    crate::image::setup().await;
    let registry = Registry::default();
    let timed = tokio::time::timeout(deadline, AssertUnwindSafe(body).catch_unwind());
    match JOURNEY_LOGS.scope(Arc::clone(&registry), timed).await {
        Ok(Ok(value)) => value,
        Ok(Err(panic)) => {
            print_logs(&registry);
            std::panic::resume_unwind(panic)
        }
        Err(_) => {
            print_logs(&registry);
            panic!("journey `{name}` did not finish within {deadline:?}")
        }
    }
}

/// Ask `probe` until it returns `Some`, or panic naming `what` when `within`
/// runs out. Every wait in a journey is a condition polled, never a bare sleep
/// sized to a guess.
///
/// # Panics
///
/// When `probe` has not returned `Some` within `within`.
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
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out after {within:?} waiting for {what}"
        );
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
