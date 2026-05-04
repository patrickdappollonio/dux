# Security Policy

This document is the **living** security posture of `dux-amq-setup`,
the Rust TUI (`dux`), and the AMQ integration shipped under
`dux-amq/`. Audit reports under `docs/audits/` are point-in-time
snapshots; this file is what stays current.

## Reporting a vulnerability

Please email `siavash@kiani.fi` with details and, if possible, a
proof-of-concept. Do **not** open a public GitHub issue for
security-relevant findings until a fix is available. We aim to
acknowledge reports within 72 hours.

## Scope

- The `dux` TUI binary built from this repository.
- The `dux-amq` installer (`dux-amq/install.sh`),
  `bashrc-additions.sh`, and the agent wrappers
  (`claude-amq`, `codex-amq`, `gemini-amq`).
- The skills and seed material installed under
  `~/.claude/skills/` and `$STATE_ROOT/`.
- The configuration and on-disk state under
  `$STATE_ROOT` (default `/data/state` on persistent-disk VMs,
  `~/.config/dux` on Linux laptops, `~/.dux` on macOS).

Out of scope: the upstream `claude`, `codex`, `gemini`, and `amq`
binaries themselves; cloud-provider IAM; the host kernel.

## Trust model summary

`dux` runs as a **single-user, single-Linux-account** TUI. All panes
spawned by `dux` share the same `$HOME`, the same filesystem
permissions, and the same environment. There is no in-VM isolation
between panes. One compromised pane = one compromised user account.

That assumption is **load-bearing** for every other mitigation in
this document. Do not deploy `dux` in a multi-tenant context where
multiple humans share the same Linux account.

## STRIDE threat model

STRIDE (Spoofing, Tampering, Repudiation, Information disclosure,
Denial of service, Elevation of privilege) covers the threats we
actively mitigate. For long-form scenarios, mitigation pointers
into source, residual risk, and detection signals, see
[`docs/operations/threat-model.md`](docs/operations/threat-model.md).

| #   | Threat                                                                                                  | STRIDE | Asset                  | Mitigation                                                            | Phase        |
|-----|---------------------------------------------------------------------------------------------------------|--------|------------------------|-----------------------------------------------------------------------|--------------|
| T1  | Malicious repo executes via `--dangerously-skip-permissions`                                            | T,E    | Host shell, API tokens | Default-deny YOLO; opt-in via `CLAUDE_AMQ_YOLO=1` / `CODEX_AMQ_YOLO=1` | 01           |
| T2  | Compromised AMQ peer spoofs `--me <other>` and injects text                                             | S,T    | Sibling panes          | HMAC-signed envelope + replay protection (per-VM secret at `$AMQ_SECRET_PATH`, default `~/.local/share/dux-amq/amq-secret`, mode 0600) | 08           |
| T3  | Tampered `amq` binary `eval`'d on every shell                                                           | T,E    | TCB                    | sha256-pinned binary + bashrc hash guard fails closed                 | 02           |
| T4  | Spot-VM preemption mid-sqlite write                                                                     | T,D    | sessions.sqlite3       | WAL journal + integrity check + periodic `.bak`                       | 14           |
| T5  | Plaintext API tokens / chat on persistent disk after VM destroyed                                       | I      | Tokens, PII            | gocryptfs / LUKS playbook in `docs/operations/encryption-at-rest.md`  | 25           |
| T6  | Right-to-erasure: per-customer chat history can't be deleted                                            | I,N    | Chat JSONLs            | `dux session purge --hard <id>` cascades to JSONLs + AMQ + sqlite     | 10           |
| T7  | Wrapper identity collision (`feat/foo` ≡ `feat-foo` after sed normalize)                                | S      | AMQ identity           | Collision detection in wrapper; fail closed with explicit error       | 22           |
| T8  | Log injection via PTY content into `dux.log`                                                            | T      | Operator trust         | `sanitize_for_terminal` strips C0/C1 control bytes                    | 03           |
| T9  | Resource exhaustion: no PTY/memory caps                                                                 | D      | Host RAM/disk          | `[limits]` config (`max_panes`, scrollback) + disk watchdog           | 16           |
| T10 | DoS via AMQ inbox flood                                                                                 | D      | Filesystem             | Rate-limit (upstream coordination); local inode monitoring            | 16, upstream |
| T11 | Symlink swap of `~/.claude` → attacker dir                                                              | T,E    | API tokens             | Symlink target check on launch (planned)                              | future       |
| T12 | Auto-resume thundering herd on spot-VM reboot                                                           | D      | Host CPU, API quota    | Bounded scheduler (`auto_resume_concurrency`, default 4) + staleness skip | 15        |
| T13 | Watch-rule regex evaluated on attacker-influenced PTY output (DoS / unintended action firing)           | T,D    | UI thread, child PTY   | Linear-time `regex` crate; per-pattern `size_limit` (64 KiB); rule cap (32/provider); per-rule `budget`/`cooldown_ms`; opt-in (commented defaults); manual disarm | 26 |

## What's in scope vs. accepted

**In scope** — we mitigate or document:

- Single-user, single-VM compromise via prompt injection.
- Supply-chain integrity of `dux`, `amq`, and the skills package
  (sha256 pins, GitHub Actions pinned by SHA, SBOM, attestation).
- Local data-at-rest exposure on persistent disks that may outlive
  the VM (operator-driven encryption playbook).
- AMQ peer-spoofing within a single VM.
- Right-to-erasure for chat history that may contain PII.

**Accepted risks** — not mitigated, with rationale:

- **Multi-tenant isolation.** dux is a single-user-on-a-VM product.
  Two humans sharing one Linux account is out of scope.
- **Cloud-provider IAM compromise.** If your GCP/AWS service
  account is hijacked, the attacker controls the disk and the VM
  before any dux-level mitigation can help.
- **Side-channel leakage from upstream CLIs.** Bugs in `claude`,
  `codex`, or `gemini` themselves (e.g. the CVE-2025/26 series) are
  the upstream vendor's responsibility. We pin their CLIs but do
  not re-implement their sandboxing.
- **TIOCSTI on legacy kernels.** Mitigation requires
  `dev.tty.legacy_tiocsti=0`; we document the sysctl in Phase 13
  but cannot guarantee its presence on every host kernel.

## Verification

The project ships a self-check tool: `dux-amq doctor` (Phase 20).
Run it after every install or upgrade:

```bash
dux-amq doctor | grep -E '(integrity|tiocsti|amq.binary|encryption)'
```

`doctor` reports:

- `binary integrity` — sha256 of `~/.local/bin/amq` against the
  recorded pin in `bashrc-additions.sh`.
- `tiocsti` — value of `dev.tty.legacy_tiocsti`.
- `sqlite integrity_check` — `PRAGMA integrity_check` on
  `sessions.sqlite3`.
- `encryption at rest` — whether the configured `$STATE_ROOT` is on
  an encrypted mount.
- AMQ queue depth, oldest-message age, `~/.claude` symlink target,
  free disk space, currently-running dux PID/uptime/RSS.

Pass `--anonymize` to redact paths and identities before sharing
the output for support.

## Update cadence

This file is **a living document**. Every audit
(`docs/audits/audit01.md`, `audit02.md`, …) must extend the STRIDE
table with new threats discovered, and stale rows must be either
re-validated or removed in the same PR that supersedes them.

When introducing a new attack surface — a new MCP integration, a
new network egress, a new file write outside `$STATE_ROOT`, a new
provider CLI — update the STRIDE table **in the same PR** that
introduces the surface. PRs that add attack surface without
updating this file are blocked at review.

A future CI check (`scripts/validate-threat-model.sh`) is planned
to compare the phase references in this table against the phase
files under `docs/plans/audits/` and warn when a phase claims to
mitigate a threat that is not listed here. Tracked as a
post-audit02 follow-up.
