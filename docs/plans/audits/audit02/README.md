# Audit 02 Implementation Plan

> Source: `docs/audits/audit02.md` (2026-05-03). 62 findings (11 P0, 29 P1, 22 P2).
> Goal: bring `dux-amq-setup` from "credible developer overlay" to **iron-clad
> production**. Resolves all 4 still-open audit01 P0s, adds 9 new P0s found
> beyond audit01's scope, and lands the architectural overhauls audit01
> deliberately deferred.

## How to use

Phases run in numerical order **by default**, but several have explicit
dependencies (see graph below) — when a phase blocks another, the blocked
phase's `Pre-conditions` lists the dep. An LLM agent picking up work should:

1. Read this README; locate the next pending phase by checking
   `Acceptance criteria` checkboxes.
2. Open that phase's file. Read **all** sections before changing code.
3. Verify the spot-checked file:line references against the current HEAD —
   audit findings were captured at `554255d`; lines may shift.
4. Implement; run **Validation**; tick **Acceptance criteria**.
5. Commit on a `audit02/<phase-slug>` branch and open a PR. Do not batch
   unrelated phases in one PR.
6. When stuck, the phase file lists `Known pitfalls` — read those first.

Effort: **S** ≤ 1 h, **M** ≈ half-day, **L** ≈ full day, **XL** ≈ 2–3 days.

## Phase index

| #  | Title                                                              | Audit IDs                          | Effort | Type |
|----|--------------------------------------------------------------------|------------------------------------|--------|------|
| 00 | Preflight: re-verify findings, baseline, scaffolding               | —                                  | S      | infra |
| 01 | Wrapper YOLO defaults (Claude + Codex) + seed default flip          | P0-A, audit01 P0-3                 | S      | shell |
| 02 | install.sh idempotency: `amq init` gate, strip_block fix, PATH     | P0-F, P0-G, P1-A, N-3              | M      | shell |
| 03 | Sanitizer module: log injection + status-line stderr               | P0-B, P0-C                         | M      | rust |
| 04 | UI-thread unblocking: route startup git through workers            | P0-D                               | M      | rust |
| 05 | pty.rs poison-tolerance + reader thread join                       | P0-E, P1-G, P1-H                   | S      | rust |
| 06 | GitHub Actions SHA pinning + permissions tightening                | P0-H, P1-N, P1-O, P1-R, P1-T       | M      | ci |
| 07 | CI security gates: cargo-audit, cargo-deny, auditable, SBOM, sign  | P0-I (full bundle)                 | L      | ci |
| 08 | AMQ message authentication (HMAC-signed envelope)                  | P0-K, T2                           | L      | shell+rust |
| 09 | `tracing` migration: structured logs, rotation, JSON layer         | P1-X                               | M      | rust |
| 10 | GDPR hard-purge: `dux session purge --hard` cascading delete       | P0-J, T6                           | M      | rust |
| 11 | Migration safety: flock, atomic swap, drop `--delete`              | audit01 P0-4                       | M      | shell |
| 12 | Path encoding + realpath cwd containment + fixture tests           | audit01 P0-5                       | M      | shell |
| 13 | TIOCSTI mitigation: detect kernel state, `--inject-via` fallback   | audit01 P1-1 (confirmed)           | M      | shell |
| 14 | sqlite hardening: WAL, integrity check, periodic backup            | P1-W                               | S      | rust |
| 15 | Auto-resume concurrency cap + staleness skip                       | P1-U, audit01 P1-3                 | S      | rust |
| 16 | Runtime resource limits: max_panes, scrollback caps, disk watchdog | P1-AA                              | M      | rust |
| 17 | App decomposition: split god-object into 6 sub-structs             | P1-V                               | XL     | rust |
| 18 | Session state machine (typestate)                                  | P1-Z                               | M      | rust |
| 19 | Schema versioning: `user_version` + migration log                  | P1-Y                               | M      | rust |
| 20 | `dux-amq doctor` triage tool                                       | audit01 P2-11                      | M      | shell+rust |
| 21 | macOS CI matrix + OsStr-based git invocation portability           | P1-S, P2-11                        | S      | ci+rust |
| 22 | Wrapper P1 hygiene bundle: setsid, preflight collect, identity     | P1-B, P1-C, P1-D, P1-E, P1-F       | M      | shell |
| 23 | Rust P1 hygiene bundle: `--`, NamedTempFile, ensure_column         | P1-I, P1-J, P1-K, P1-L, P1-M       | M      | rust |
| 24 | P2 bundle: CODEOWNERS, SECURITY.md, dependabot, AMQ TTL, profile   | P2-1..21, P2-22                    | M      | meta |
| 25 | Encryption-at-rest playbook (operator docs)                        | T5, P2-21                          | S      | docs |
| 26 | Threat model docs: SECURITY.md with explicit STRIDE                | (audit02 §threat-model)            | S      | docs |
| 27 | Final validation: full gate run, kernel matrix smoke, doctor dump  | All                                | M      | infra |

## Dependency graph

```
00 ─┬─► 01 ─┐
    ├─► 02 ─┤
    ├─► 03 ─┼─► 04, 05, 09 ─┐
    ├─► 06 ─►─ 07 ───────── │
    ├─► 11, 12, 13          │
    │                       │
    │   09 ───► 10, 20, 24   │
    │   17 ───► 18, 19      │  ───► 27 (final)
    │   14 ───► 16          │
    │   15, 21, 22, 23, 25, 26
```

Phase 00 is foundation; everything else depends on it. **Phase 09
(`tracing`) is the most-shared dependency** — Phase 10 (purge) and Phase 20
(doctor) consume structured fields it produces. Land 09 before either.
**Phase 17 (App decomposition)** is the largest single change and should
ship on its own week so 18 and 19 can rebase cleanly onto it.

## Sequencing recommendation (one engineering week)

**Day 1 (security P0s)**: 01, 03, 06 — wrapper defaults, sanitizer, GHA pinning. Three small PRs, all merge-ready by EOD.

**Day 2 (install + CI)**: 02, 07, 11. install.sh idempotency, full CI gate bundle, migration safety. 07 is the big one; pair with reviewer.

**Day 3 (Rust P0s)**: 04, 05, 09. UI-thread unblocking, pty hardening, `tracing` migration. 09 unblocks Day 4.

**Day 4 (compliance + UX)**: 10, 12, 13, 20. GDPR purge, path encoding, TIOCSTI mitigation, doctor tool.

**Day 5 (P1 cleanup + validation)**: 08, 14, 15, 16, 21, 22, 23, 27. Bundle smaller P1s and final validation.

**Week 2 (overhauls)**: 17, 18, 19. The architectural changes. Single feature branch, multiple PRs.

**Backlog (when bandwidth)**: 24, 25, 26.

## Known pitfalls / verification points

- **Phase 06**: `pinact`/`frizbee` rewrites can churn unrelated workflow lines. Pin one workflow at a time and review the diff manually.
- **Phase 07**: `cargo deny` has a steep first-run learning curve — start with the [example deny.toml](https://github.com/EmbarkStudios/cargo-deny/blob/main/deny.toml) and iterate. Fail-on `[advisories]` first; license/bans can warn-only initially.
- **Phase 08**: AMQ HMAC needs upstream coordination. If `agent-message-queue` rejects the proposal, fall back to wrapping `amq send`/`amq read` in our own envelope tool.
- **Phase 09**: switching to `tracing` mid-stream means `logger::error/info` become macros under the hood. Watch for `&format!(...)` patterns that should become `error!(field=%value, "msg")`.
- **Phase 13**: TIOCSTI is **confirmed** to be the only AMQ injection path (audit02 verified `wake_tiocsti_unix.go`). Plan B (`--inject-via tmux send-keys`) requires tmux or a small dux-side bridge.
- **Phase 17**: God-object decomposition creates merge conflicts with anything in flight. Freeze other Rust PRs while it lands.

## Out of scope

- Migration to Claude Code `auto mode` (upstream still in transition; revisit when wrapper plumbing supports `--auto`).
- Multi-user / multi-tenant isolation (single-user model is a documented assumption).
- AMQ binary internals beyond the wake transport (separate audit).
- Windows native (WSL2 only, per CLAUDE.md).

## References

- audit02 source: `docs/audits/audit02.md`
- audit01 source: `docs/audits/audit01.md`
- audit01 plan (precedent): `docs/plans/audits/audit01/`
