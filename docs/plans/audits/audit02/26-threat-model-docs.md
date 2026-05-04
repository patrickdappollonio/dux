# Phase 26: Threat model docs — SECURITY.md with explicit STRIDE

> Maps to: audit02 §threat-model (consolidate into the repo).

## Goal
Promote the audit02 threat-model section into a living `SECURITY.md`
document the project owns and updates as new threats emerge. The audit
docs are point-in-time; the security posture must be perpetually
current.

## Pre-conditions
- Phase 00 baseline green.
- Phase 24 (basic SECURITY.md skeleton) merged — this phase fleshes it out.
- Ideally Phase 08 (HMAC), Phase 13 (TIOCSTI), Phase 25 (encryption)
  merged — they're referenced here as mitigations.

## Files to touch
- `SECURITY.md` — extend from Phase 24 minimal version.
- `docs/operations/threat-model.md` — NEW (long-form companion).

## Steps

### 26.1 — Extend SECURITY.md with the threat model
Below the existing "Reporting" + "Scope" + "Trust model summary"
sections (Phase 24), add:

```markdown
## STRIDE threat model

| #  | Threat                                                                                                  | STRIDE | Asset                  | Mitigation                                | Phase |
|----|---------------------------------------------------------------------------------------------------------|--------|------------------------|-------------------------------------------|-------|
| T1 | Malicious repo executes via `--dangerously-skip-permissions`                                            | T,E    | Host shell, API tokens | Default-deny YOLO; opt-in via env var     | 01 |
| T2 | Compromised AMQ peer spoofs `--me <other>` and injects text                                             | S,T    | Sibling panes          | HMAC-signed envelope + replay protection  | 08 |
| T3 | Tampered `amq` binary `eval`'d on every shell                                                           | T,E    | TCB                    | sha256-pinned binary + bashrc hash guard  | 02 |
| T4 | Spot-VM preemption mid-sqlite write                                                                     | T,D    | sessions.sqlite3       | WAL journal + integrity check + .bak      | 14 |
| T5 | Plaintext API tokens / chat on persistent disk after VM destroyed                                       | I      | Tokens, PII            | gocryptfs / LUKS playbook                 | 25 |
| T6 | Right-to-erasure: per-customer chat history can't be deleted                                            | I,N    | Chat JSONLs            | `dux session purge --hard`                | 10 |
| T7 | Wrapper identity collision (`feat/foo` ≡ `feat-foo` after sed normalize)                                | S      | AMQ identity           | Collision detection in wrapper            | 22 |
| T8 | Log injection via PTY content into `dux.log`                                                            | T      | Operator trust         | Sanitizer module                          | 03 |
| T9 | Resource exhaustion: no PTY/memory caps                                                                 | D      | Host RAM/disk          | `[limits]` config + disk watchdog         | 16 |
| T10| DoS via AMQ inbox flood                                                                                 | D      | Filesystem             | Rate-limit (upstream); inode monitoring   | 16, upstream |
| T11| Symlink swap of `~/.claude` → attacker dir                                                              | T,E    | API tokens             | Symlink target check on launch (TODO)     | future |
| T12| Auto-resume thundering herd                                                                             | D      | Host CPU               | Bounded scheduler + staleness skip        | 15 |

## What's in scope vs. accepted

**In scope** — we mitigate or document:
- Single-user, single-VM compromise via prompt injection.
- Supply-chain integrity of `dux`, `amq`, skills package.
- Local data-at-rest exposure (with operator-driven encryption).
- AMQ peer-spoofing within one VM.

**Accepted risks**:
- Multi-tenant isolation. The product is single-user-on-a-VM.
- Cloud-provider IAM compromise (out of our threat model).
- Side-channel leakage from the `claude` / `codex` / `gemini` CLIs themselves.

## Verification

`dux-amq doctor` (Phase 20) reports current security posture in one
place. Run after every install:
```
dux-amq doctor | grep -E '(integrity|tiocsti|amq.binary|encryption)'
```

## Update cadence

Every audit (audit01, audit02, …) must extend this file with new
threats discovered. Stale rows must be either re-validated or removed
in the same PR that supersedes them.
```

### 26.2 — Long-form companion at `docs/operations/threat-model.md`
For the rows above, write a paragraph each describing:
- Concrete attack scenario (1–3 sentences).
- Mitigation in code (file:line refs).
- Residual risk after mitigation.
- Detection (what shows up in `dux.log` / doctor output).

This is the artifact a security reviewer reads; SECURITY.md is the
quick-reference table.

### 26.3 — Cross-references
- Each phase file in `docs/plans/audits/audit02/` — add a "Threat
  refs: T1, T8" line to the front matter.
- `docs/audits/audit02.md` — link to the canonical threat model in
  `SECURITY.md` (the audit becomes a frozen snapshot; SECURITY.md is
  the living document).

### 26.4 — Update cadence in CONTRIBUTING
Append to CLAUDE.md or a CONTRIBUTING.md:
> When introducing a new attack surface (new MCP integration, new
> network egress, new file write outside `$STATE_ROOT`), update
> `SECURITY.md` STRIDE table in the same PR. Threat model is a
> living document; PRs that introduce attack surface without
> updating it are blocked.

## Validation
- Manual review by a second engineer; treat as a security review.
- `markdownlint` clean.

## Acceptance criteria
- [x] `SECURITY.md` has a STRIDE table covering T1–T12.
- [x] `docs/operations/threat-model.md` exists with one paragraph per row.
- [x] Audit02 plan phase files cross-reference threat IDs (T1–T12 referenced from the relevant phase files).
- [x] CLAUDE.md updated with the "must update SECURITY.md
      when adding attack surface" rule (`Update SECURITY.md when you add attack surface`).
- [x] PR: `docs(security): living STRIDE threat model + update cadence` — landed via PR #2.

## Known pitfalls
- The threat model will go stale fast unless engineers actually
  update it. Pair with a CI check: a `validate-threat-model.sh` that
  compares STRIDE-table phase refs against the phase files; warn
  if a phase claims to mitigate a threat that's not in SECURITY.md.
- Don't speculate threats you have no plan to address — accepted
  risks belong under "Accepted" with rationale.

## References
- audit02 §threat-model.
- STRIDE: Microsoft Threat Modeling Tool docs.
- LINDDUN (privacy-extended STRIDE): https://linddun.org/
