//! End-to-end integration test for the watch engine.
//!
//! Spawns `cat` in a real PTY (via `dux::pty::PtyClient`), writes a
//! pattern-matching string into the PTY, scans the resulting visible
//! viewport with `WatchEngine`, and asserts that the engine's
//! `WatchEffect::SendText` payload — when written back through the PTY —
//! shows up in the next scan. This exercises the full Phase 1 surface:
//! regex matching, backoff/cooldown timing, the scan helper on
//! `PtyClient`, and the integration between effects and PTY writes.

use std::thread;
use std::time::{Duration, Instant};

use dux::pty::PtyClient;
use dux::watch::{WatchAction, WatchBackoff, WatchBudget, WatchEffect, WatchEngine, WatchRule};

/// Poll `cond` until it returns true or `timeout` elapses. Sleeps for
/// `step` between polls. Returns true if the condition held within the
/// budget. Used so the test isn't a flaky fixed-sleep.
fn wait_until<F: FnMut() -> bool>(mut cond: F, timeout: Duration, step: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        thread::sleep(step);
    }
    cond()
}

fn rule_matching(pattern: &str, text: &str) -> WatchRule {
    WatchRule {
        pattern: pattern.to_string(),
        label: "test rule".to_string(),
        action: WatchAction::SendText {
            text: text.to_string(),
            append_enter: true,
        },
        backoff: WatchBackoff {
            // Tight backoff so the test is fast but still exercises the
            // Idle → Pending → fire transition.
            initial_ms: 50,
            max_ms: 1_000,
            multiplier: 2.0,
            jitter_ms: 0,
        },
        budget: WatchBudget { max_attempts: 3 },
        cooldown_ms: 100,
    }
}

#[test]
fn watch_engine_matches_pty_output_and_drives_send_text() {
    // Spawn `cat` in a small PTY. cat echoes everything we write back.
    let cwd = std::env::temp_dir();
    let client = PtyClient::spawn("cat", &[], &cwd, 24, 80, 1_000).expect("spawn cat in PTY");

    // Write a string that matches the watch rule's pattern.
    client
        .write_bytes(b"the agent is rate limited now\r")
        .expect("write trigger to PTY");

    // Wait for cat to echo the line.
    let saw_match = wait_until(
        || client.scan_recent_lines(30).contains("rate limited"),
        Duration::from_secs(2),
        Duration::from_millis(20),
    );
    assert!(
        saw_match,
        "cat should echo trigger string within 2s; actual: {:?}",
        client.scan_recent_lines(30)
    );

    // Build the engine with the throttle-style rule and a fast backoff.
    let (mut engine, errors) = WatchEngine::new(
        "session-cat".to_string(),
        &[rule_matching("rate limited", "please continue")],
    );
    assert!(errors.is_empty(), "rule load errors: {errors:?}");
    assert_eq!(engine.rule_count(), 1);

    // First observe: should detect the match and schedule a fire ~50ms
    // out. No effects yet.
    let snapshot = client.scan_recent_lines(30);
    let effects = engine.observe(&snapshot, Instant::now());
    assert!(
        effects.is_empty(),
        "first observe should only schedule, not fire: {effects:?}"
    );

    // Wait past the backoff window and observe again to fire. Drain a
    // few frames in case the backoff jitter pushes us slightly past the
    // first poll.
    let mut send_text: Option<String> = None;
    let mut status_seen = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && send_text.is_none() {
        thread::sleep(Duration::from_millis(40));
        let snapshot = client.scan_recent_lines(30);
        for effect in engine.observe(&snapshot, Instant::now()) {
            match effect {
                WatchEffect::SendText { text, append_enter } => {
                    assert!(append_enter, "rule was configured with append_enter=true");
                    send_text = Some(text);
                }
                WatchEffect::StatusInfo(_) => status_seen = true,
                WatchEffect::StatusWarning(msg) => panic!("unexpected warning: {msg}"),
            }
        }
    }
    let send_text = send_text.expect("engine should have fired SendText within 2s");
    assert_eq!(send_text, "please continue");
    assert!(
        status_seen,
        "engine should have emitted StatusInfo when firing"
    );

    // Apply the SendText effect: write payload + CR to the PTY. cat
    // should echo it back. This proves the full "watch fires → bytes
    // hit the agent" pathway works end to end.
    let mut payload = send_text.into_bytes();
    payload.push(b'\r');
    client.write_bytes(&payload).expect("send retry to PTY");
    let saw_retry = wait_until(
        || client.scan_recent_lines(30).contains("please continue"),
        Duration::from_secs(2),
        Duration::from_millis(20),
    );
    assert!(
        saw_retry,
        "cat should echo the watch-engine payload; actual: {:?}",
        client.scan_recent_lines(30)
    );
}

#[test]
fn watch_engine_no_rules_for_provider_is_noop() {
    // With zero rules, the engine reports rule_count == 0 and observe
    // returns an empty Vec for any snapshot.
    let (mut engine, errors) = WatchEngine::new("noop", &[]);
    assert!(errors.is_empty());
    assert_eq!(engine.rule_count(), 0);

    let effects = engine.observe("rate limited everything is on fire", Instant::now());
    assert!(effects.is_empty(), "no rules ⇒ no effects: {effects:?}");
}
