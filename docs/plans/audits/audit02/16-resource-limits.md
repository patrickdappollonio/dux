# Phase 16: Runtime resource limits — max_panes, scrollback caps, disk watchdog

> Maps to: **P1-AA**.

## Goal
Cap host resources dux can consume: maximum number of panes/companion
terminals, total scrollback memory across panes, and refuse new agent
spawns when persistent disk crosses high-water marks. Today nothing
prevents 100 panes × 1 MB grid + 100 MB Claude process from OOMing the
host in under a minute.

## Pre-conditions
- Phase 00 baseline green.
- Phase 14 (sqlite WAL) merged — disk watchdog uses sqlite for the
  history table.

## Files to touch
- `src/config.rs` — `[limits]` section.
- `src/app/sessions.rs` — refuse spawn when caps exceeded.
- `src/app/workers.rs` — disk-usage sampler worker.
- `src/app/mod.rs` — display banner when limits hit.
- `tests/limits.rs` — NEW.

## Steps

### 16.1 — Config
```rust
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct LimitsConfig {
    /// Maximum simultaneous agent panes. 0 = unlimited (NOT recommended). Default 16.
    #[serde(default = "default_max_panes")]
    pub max_panes: usize,
    /// Maximum companion (raw shell) terminals. Default 4.
    #[serde(default = "default_max_companions")]
    pub max_companion_terminals: usize,
    /// Soft cap on total scrollback grid memory across all panes (MiB).
    /// When exceeded, oldest panes are auto-detached (not killed). Default 256.
    #[serde(default = "default_max_scrollback_mb")]
    pub max_total_scrollback_mb: usize,
    /// When persistent-disk usage exceeds this percentage, refuse new
    /// agent spawns. Default 95.
    #[serde(default = "default_disk_high_water")]
    pub disk_high_water_pct: u8,
    /// Warn (status line) when disk exceeds this percentage. Default 80.
    #[serde(default = "default_disk_warn")]
    pub disk_warn_pct: u8,
}
```

### 16.2 — Spawn refusal at limit
In `App::create_agent`:
```rust
fn create_agent(&mut self, ...) -> anyhow::Result<()> {
    let active_panes = self.sessions.iter()
        .filter(|s| s.status == SessionStatus::Active)
        .count();
    let max = self.config.limits.max_panes;
    if max > 0 && active_panes >= max {
        self.set_error(format!(
            "Refusing new agent: {active_panes} panes already running (config limits.max_panes = {max}). \
             Detach an unused pane or raise the cap."));
        anyhow::bail!("max_panes reached");
    }
    if self.disk_usage_pct() >= self.config.limits.disk_high_water_pct {
        self.set_error(format!(
            "Refusing new agent: persistent disk at {}% (limits.disk_high_water_pct = {}%). \
             Run `dux session purge` or extend the disk.",
            self.disk_usage_pct(),
            self.config.limits.disk_high_water_pct));
        anyhow::bail!("disk full");
    }
    // ... existing creation path ...
}
```

### 16.3 — Auto-detach on scrollback overflow
Sample the total scrollback footprint approximately once per minute
in a worker, calculating `panes.iter().map(|p| p.scrollback_lines * cols * 4).sum()`.
When over `max_total_scrollback_mb`, detach oldest-by-`last_active_at`
panes until under the cap. Detach != kill — it just frees the in-memory grid.

### 16.4 — Disk-usage sampler
`src/app/workers.rs`:
```rust
pub fn spawn_disk_watchdog(paths: DuxPaths, tx: WorkerSender, cfg: LimitsConfig) {
    std::thread::Builder::new().name("disk-watchdog".into()).spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(60));
            if let Ok(stat) = nix::sys::statvfs::statvfs(&paths.root) {
                let total = stat.blocks() as u64 * stat.fragment_size() as u64;
                let avail = stat.blocks_available() as u64 * stat.fragment_size() as u64;
                let used_pct = ((total - avail) * 100 / total.max(1)) as u8;
                let _ = tx.send(WorkerEvent::DiskUsage(used_pct));
            }
        }
    }).ok();
}
```
(`nix` may not be in deps; use `rustix::fs::statvfs` instead — already a dep.)

### 16.5 — Status line / banner
On `WorkerEvent::DiskUsage(pct)`:
- pct ≥ disk_high_water_pct → red banner "Disk at NN%; new agents refused"
- pct ≥ disk_warn_pct → yellow status "Disk at NN%; consider purge"
- else → clear

### 16.6 — Tests
`tests/limits.rs`:
```rust
#[test]
fn create_agent_refused_at_max_panes() {
    let mut app = test_app_with_limit(2);
    app.fake_active_sessions(2);
    let result = app.create_agent(...);
    assert!(result.is_err());
    assert!(app.status_line.contains("max_panes"));
}
#[test]
fn disk_high_water_refuses_spawn() { ... }
#[test]
fn scrollback_overflow_detaches_oldest_pane() { ... }
```

## Validation
- `cargo test limits` green.
- Manual: set `max_panes = 2`; create 3 agents; third refused with
  helpful status line.
- Manual: fill `/data` to 95%+ (`dd` zeros into a tmp file); attempt
  agent creation; refused.

## Acceptance criteria
- [ ] `LimitsConfig` with 5 fields rendered in canonical config.
- [ ] `create_agent` checks `max_panes` and `disk_high_water_pct`.
- [ ] `disk_watchdog` worker emits `DiskUsage` events every 60 s.
- [ ] Status line shows banner at warn/high-water.
- [ ] Auto-detach on scrollback overflow implemented (or feature-gated
      behind `[limits]` if too risky for first land).
- [ ] 3 tests pass.
- [ ] PR: `feat(limits): pane/scrollback/disk caps (P1-AA)`.

## Known pitfalls
- `statvfs` on a bind mount may report the underlying disk's stats
  rather than the bind point's. Test on a real /data setup.
- `max_panes = 0` semantics: pick "unlimited" or "zero allowed"
  consistently. Default to "unlimited" (= disabled), document.
- Auto-detach is destructive in spirit — operators may complain when
  their long-running agent gets detached during a meeting. Consider
  a confirmation prompt or an opt-out env var.
- Disk usage calc: persistent disk root is `/data`, but dux's `paths.root`
  may be a sub-path. Use `statvfs(paths.root)` and accept it returns
  the underlying filesystem.

## References
- audit02 P1-AA.
- `rustix::fs::statvfs`: https://docs.rs/rustix/latest/rustix/fs/fn.statvfs.html
