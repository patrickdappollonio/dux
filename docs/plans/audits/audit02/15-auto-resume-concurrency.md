# Phase 15: Auto-resume concurrency cap + staleness skip

> Maps to: **P1-U** (audit02), **audit01 P1-3** (still open).

## Goal
Today `auto_resume_all_sessions` (`src/app/mod.rs:1380-1410`) loops
through every persisted session and spawns a PTY synchronously,
unbounded. 50 sessions = 50 simultaneous Anthropic TLS handshakes,
which the API rate-limits — half the panes exit immediately and the
status line thrashes. On spot-VM reboot this becomes a fork-bomb.

Add a bounded concurrency cap, a per-session backoff, and a staleness
filter so worktrees untouched for N days don't auto-spawn at all.

## Pre-conditions
- Phase 00 baseline green.
- Phase 04 (UI thread workers) merged — auto-resume already runs in
  workers but the scheduler is the issue.

## Files to touch
- `src/app/sessions.rs` (or wherever `auto_resume_all_sessions` lives) — bound the fan-out.
- `src/config.rs` — new fields.
- `src/model.rs` — track `last_active_at` if not already.
- `src/app/workers.rs` — coordinated spawn scheduler.
- `tests/auto_resume.rs` — NEW.

## Steps

### 15.1 — Config
```rust
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct AutoResumeConfig {
    /// Max number of sessions spawned in parallel during auto-resume.
    /// Lower = slower startup, less API load. Default 4.
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    /// Skip sessions whose worktree was last modified > N days ago.
    /// 0 disables the filter. Default 30.
    #[serde(default = "default_stale_days")]
    pub stale_days: u32,
    /// Stagger delay between successive spawns (ms). Default 250.
    #[serde(default = "default_stagger_ms")]
    pub stagger_ms: u64,
}
fn default_concurrency() -> usize { 4 }
fn default_stale_days() -> u32 { 30 }
fn default_stagger_ms() -> u64 { 250 }
```

### 15.2 — Bounded scheduler
Replace the loop with a worker that pulls from a queue with a semaphore:
```rust
fn auto_resume_all_sessions(&mut self) {
    let candidates: Vec<_> = self.sessions.iter()
        .filter(|s| s.status != SessionStatus::Active)
        .filter(|s| !is_stale(&s.worktree, self.config.auto_resume.stale_days))
        .map(|s| s.id.clone())
        .collect();

    let cfg = self.config.auto_resume.clone();
    let tx = self.worker_tx.clone();
    let store = self.storage.clone();

    std::thread::Builder::new().name("auto-resume".into()).spawn(move || {
        // Bound parallelism with a counting Semaphore (we have std::sync only;
        // use a Condvar+Mutex<usize> miniature semaphore).
        let permits = Arc::new((Mutex::new(cfg.concurrency), Condvar::new()));
        for (i, id) in candidates.into_iter().enumerate() {
            // Stagger
            std::thread::sleep(Duration::from_millis(cfg.stagger_ms));
            // Acquire
            let permits_clone = permits.clone();
            {
                let (lock, cv) = &*permits_clone;
                let mut g = lock.lock().unwrap();
                while *g == 0 { g = cv.wait(g).unwrap(); }
                *g -= 1;
            }
            let tx = tx.clone();
            let store = store.clone();
            std::thread::spawn(move || {
                let _result = spawn_one_session(&store, &id);
                let _ = tx.send(WorkerEvent::SessionSpawned { id, /* ... */ });
                // Release
                let (lock, cv) = &*permits_clone;
                let mut g = lock.lock().unwrap();
                *g += 1;
                cv.notify_one();
            });
        }
    }).ok();
}
```

A simpler alternative: use a `crossbeam-channel`-backed bounded
queue with `cfg.concurrency` workers. Either is fine; the former
keeps zero dep additions.

### 15.3 — Staleness filter
```rust
fn is_stale(worktree: &Path, days: u32) -> bool {
    if days == 0 { return false; }
    let Ok(meta) = std::fs::metadata(worktree) else { return false; };
    let Ok(mtime) = meta.modified() else { return false; };
    let age = std::time::SystemTime::now()
        .duration_since(mtime).unwrap_or_default();
    age.as_secs() > (days as u64) * 86400
}
```

### 15.4 — Tests
`tests/auto_resume.rs`:
```rust
#[test]
fn at_most_concurrency_workers_at_once() {
    // Mock spawn_one_session to record concurrency peak via shared atomic.
    // Run with cfg.concurrency = 3, 10 candidates.
    // Assert peak <= 3.
}

#[test]
fn stale_sessions_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let stale = tmp.path().join("old");
    std::fs::create_dir(&stale).unwrap();
    set_mtime_days_ago(&stale, 60).unwrap();
    assert!(is_stale(&stale, 30));
}

#[test]
fn stagger_introduces_minimum_delay() {
    // Spawn N candidates, time-stamp each spawn entry.
    // Assert delta(i, i+1) >= stagger_ms.
}
```

## Validation
- `cargo test auto_resume` green.
- Manual: configure 20 sessions, set `auto_resume.concurrency = 4`;
  observe at most 4 PTYs being spawned at once via `pgrep -fc claude`.
- Disk-mtime test: touch a worktree to 60 days ago; observe
  `auto_resume` skips it with a debug log line.

## Acceptance criteria
- [x] `AutoResumeConfig` with `concurrency`, `stale_days`, `stagger_ms`.
- [x] `auto_resume_all_sessions` uses a bounded scheduler.
- [x] Stale filter applied; returns to count of skipped sessions in log.
- [x] 3 unit/integration tests pass (`tests/auto_resume.rs`).
- [x] No new deps added (std + existing crossbeam).
- [x] Canonical config rendered with `[auto_resume]` section + comments.
- [x] PR: `perf(auto-resume): bounded concurrency + staleness filter (P1-U)` — landed via PR #2.

## Known pitfalls
- `Condvar::wait` can spuriously wake; always re-check `*g == 0` in
  `while`, not `if`.
- A panic inside `spawn_one_session` must not leak the permit. Wrap
  the release in a guard struct that releases on `Drop`.
- `set_mtime_days_ago` in tests requires `filetime` dep or libc
  `utimes`. Either is fine; prefer existing dep `filetime` if present.
- Default of 4 is conservative — may need to bump to 8 once Phase 14
  WAL reduces sqlite write contention.

## References
- audit02 P1-U.
- audit01 P1-3.
