//! Integration tests for the PTY raw-byte fan-out. These spawn a real PTY via
//! the public `PtyClient`.

use std::path::Path;
use std::time::{Duration, Instant};

use dux_core::pty::PtyClient;

fn drain_until(rx: &std::sync::mpsc::Receiver<Vec<u8>>, marker: &str, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    let mut acc: Vec<u8> = Vec::new();
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(chunk) => {
                acc.extend_from_slice(&chunk);
                if String::from_utf8_lossy(&acc).contains(marker) {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    String::from_utf8_lossy(&acc).into_owned()
}

#[test]
fn subscriber_receives_live_pty_bytes() {
    // `cat` echoes stdin back to stdout. Subscribe BEFORE writing so there is no
    // race against the streamed output.
    let client = PtyClient::spawn("cat", &[], Path::new("/"), 24, 80, 1000).expect("spawn cat");
    let (_guard, rx) = client.subscribe();
    client.write_bytes(b"ping-fanout\n").expect("write");

    let seen = drain_until(&rx, "ping-fanout", Duration::from_secs(5));
    assert!(
        seen.contains("ping-fanout"),
        "did not see marker, saw: {seen:?}"
    );
}

#[test]
fn repaint_contains_current_screen_contents() {
    // `printf` writes a marker that lands in the grid; after a moment, a repaint
    // of the current screen must contain it.
    let client = PtyClient::spawn(
        "printf",
        &["repaint-content".to_string()],
        Path::new("/"),
        24,
        80,
        1000,
    )
    .expect("spawn printf");
    std::thread::sleep(Duration::from_millis(400));

    let (_guard, repaint, _rx) = client.subscribe_with_repaint();
    let text = String::from_utf8_lossy(&repaint);
    assert!(
        text.contains("repaint-content"),
        "repaint missing screen content: {text:?}"
    );
}
