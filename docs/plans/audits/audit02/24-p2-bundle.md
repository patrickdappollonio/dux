# Phase 24: P2 bundle — CODEOWNERS, SECURITY.md, dependabot, AMQ TTL, build profile, et al.

> Maps to: **P2-1** through **P2-22** (selected). The "kitchen sink" cleanup phase.

## Goal
Land the small-but-numerous P2 hygiene items as one PR. None are
production-blockers; the cumulative effect is meaningful.

## Pre-conditions
- Phase 00 baseline green.
- Phase 06 (GHA pinning) merged — dependabot config refers to pinned actions.

## Files to touch
- `.github/CODEOWNERS` — NEW.
- `.github/dependabot.yml` — NEW.
- `SECURITY.md` — NEW.
- `Cargo.toml` — `[profile.release]` `debug = "line-tables-only"`.
- `dux-amq/install.sh` — `printf` instead of `echo`, named TMP vars,
  `chmod`/atomic for VSCode jq.
- `dux-amq/wrappers/*` — `printf '%s'` for paths (P2-3).
- `release.yml` — `concurrency:`, reproducible-tar flags (P2-6/7),
  pinned macos versions (P2-5), README hard-fail (P2-8).
- `pr.yml`, `overlay-ci.yml` — scope `push:` to `main` (P2-9).
- `src/storage.rs` — log warns on garbled JSON (P2-10).
- `src/config.rs` — post-expansion canonicalisation (P2-14).
- `tests/scrollbar_render.rs` — populate or delete (P2-19).

## Steps

### 24.1 — `.github/CODEOWNERS`
```
# Default owner for the repository
*                              @SiavZ

# Rust patches to upstream files require sharper review
src/clipboard.rs               @SiavZ
src/app/mod.rs                 @SiavZ
src/app/render.rs              @SiavZ
src/config.rs                  @SiavZ

# Audit deliverables
docs/audits/                   @SiavZ
docs/plans/audits/             @SiavZ

# Supply-chain critical
.github/workflows/             @SiavZ
dux-amq/install.sh             @SiavZ
deny.toml                      @SiavZ
rust-toolchain.toml            @SiavZ
```

### 24.2 — `SECURITY.md`
```markdown
# Security policy

This repository is `dux-amq-setup`, a fork of patrickdappollonio/dux
overlaid with the dux-amq toolset. We treat security findings seriously.

## Reporting a vulnerability
Email siavash@kiani.fi or open a *private* GitHub Security Advisory.
Do not file public issues for vulnerabilities.

## Scope
- The `dux-amq/` overlay (wrappers, install.sh, config, scripts).
- The Rust patches in `src/{clipboard.rs, app/mod.rs, app/render.rs, config.rs}`.
- Our CI pipelines under `.github/workflows/`.

## Out of scope (upstream)
- `dux` itself beyond the four patched files.
- `agent-message-queue` (the `amq` binary) — file at
  https://github.com/avivsinai/agent-message-queue.
- The upstream Claude Code, Codex, and Gemini CLIs.

## Trust model summary
- Single-user, single-Linux-account on a persistent-disk VM.
- Pinned (sha256) supply chain for dux + amq + skills.
- HMAC-signed envelope between AMQ peers (Phase 8).
- Default-deny on tool execution (no YOLO without explicit env var).
- See `docs/audits/audit02.md` §threat-model for the full STRIDE table.

## Known limitations
- No multi-user isolation.
- No application-level encryption at rest beyond cloud-provider KMS.
  Use LUKS or gocryptfs for stronger guarantees (see operator playbook).
```

### 24.3 — `.github/dependabot.yml`
```yaml
version: 2
updates:
  - package-ecosystem: "cargo"
    directory: "/"
    schedule: { interval: "weekly", day: "monday" }
    open-pull-requests-limit: 5
    allow:
      - dependency-type: "direct"
      - dependency-type: "indirect"
    groups:
      patches:
        update-types: ["patch"]
  - package-ecosystem: "github-actions"
    directory: "/"
    schedule: { interval: "weekly", day: "monday" }
    open-pull-requests-limit: 5
```
Dependabot will produce SHA-pinned action upgrade PRs (combining well
with Phase 06's `pinact` baseline).

### 24.4 — Build profile
`Cargo.toml`:
```toml
[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
debug = "line-tables-only"   # was 0; minor size cost, big diagnostics win
strip = true                 # explicit; strips symbols at link
```
Tests: build a release binary; `panic!()` somewhere; verify backtrace
includes file:line.

### 24.5 — install.sh nits
- `:24` — drop `cd -`, use absolute paths.
- All `echo "$VAR"` for paths → `printf '%s\n' "$VAR"`.
- VSCode jq merge: preserve original mode via `chmod --reference="$f.tmp" "$f"`
  before mv.

### 24.6 — Wrapper printf
`claude-amq:38-39` etc. — already covered by Phase 12 path encoding
fix. If Phase 12 isn't done yet, do here:
```bash
enc_self=$(printf '%s' "$PWD" | sed ...)
```

### 24.7 — release.yml
- `concurrency: { group: release-${{ github.event.release.tag_name }}, cancel-in-progress: false }` at top level.
- `tar --sort=name --owner=0 --group=0 --numeric-owner --mtime='@${{ github.event.release.created_at }}'`.
- Matrix: pin `macos-13` Intel + `macos-14` ARM (already in Phase 21).
- README parsing: `[[ -n "$install_section" ]] || { echo "no Install section"; exit 1; }`.

### 24.8 — pr.yml / overlay-ci.yml `on:` scoping
```yaml
on:
  pull_request:
  push:
    branches: [main]
```

### 24.9 — Storage warns on corrupt JSON
`src/storage.rs:243-249`. Replace `unwrap_or_else(|_| ...)` with:
```rust
match serde_json::to_string(&providers) {
    Ok(s) => s,
    Err(e) => {
        tracing::warn!(target: "dux::storage", err = %e, "providers serialize failed; persisting []");
        "[]".to_string()
    }
}
```
Same for `parse_started_providers` — `match serde_json::from_str` and warn.

### 24.10 — Config path expansion canonicalize
`src/config.rs::expand_path` — after `${X}` expansion, run a
`.canonicalize()` check (or strip `..` components on the expanded
result). Document in the function header.

### 24.11 — `tests/scrollbar_render.rs`
Per audit02 P2-19: file is a placeholder. Either populate (Phase 10
of audit01 plan covers this — confirm) or delete. Decide; act; don't
leave it dangling.

## Validation
- `cargo test` green.
- `gh pr checks` green; dependabot lint passes.
- Manual: trigger a `panic!()` in a test build; verify file:line in
  the backtrace (Phase 24.4).

## Acceptance criteria
- [ ] `.github/CODEOWNERS` covers patched files + audit deliverables + supply chain.
- [ ] `SECURITY.md` lives at repo root.
- [ ] `.github/dependabot.yml` covers cargo + github-actions weekly.
- [ ] `Cargo.toml` `[profile.release]` has `debug = "line-tables-only"`.
- [ ] `install.sh` uses `printf '%s'` for paths; preserves mode on jq merge.
- [ ] `release.yml` has `concurrency:`, reproducible-tar flags, README hard-fail.
- [ ] `pr.yml` and `overlay-ci.yml` scope push to main.
- [ ] `storage.rs` warns instead of silent fallback on JSON failures.
- [ ] `expand_path` post-expansion `..` rejection.
- [ ] `tests/scrollbar_render.rs` either populated or removed.
- [ ] PR: `chore(p2): hygiene bundle — CODEOWNERS, SECURITY, dependabot, build profile, et al.`.

## Known pitfalls
- `debug = "line-tables-only"` may not be available on all rustc
  channels; verify against `rust-toolchain.toml`'s pinned version.
- Dependabot config schema occasionally bumps; verify against
  https://docs.github.com/.../dependabot-options-reference.
- CODEOWNERS only takes effect when branch protection is configured
  (Phase 26 docs); without it the file is informational.
- `expand_path` post-canonicalize may break legitimate uses where
  the path doesn't yet exist (canonicalize fails on missing paths).
  Use a manual `..`-stripping pass instead.

## References
- audit02 P2-1..22.
- GitHub CODEOWNERS docs.
- Dependabot config reference.
