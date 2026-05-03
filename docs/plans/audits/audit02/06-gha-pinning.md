# Phase 06: GitHub Actions SHA pinning + permissions tightening

> Maps to: **P0-H** (tag-pinned actions), **P1-N** (rust-cache poisoning), **P1-O** (workflow-level permissions too broad), **P1-R** (no `permissions:` on PR/test workflows), **P1-T** (shellcheck doesn't lint top-level install.sh + bashrc-additions.sh).

## Goal
Pin every third-party GitHub Action by commit SHA (not tag), tighten
`permissions:` to job-level read-only by default, and ensure shellcheck
covers every shell script we ship.

## Pre-conditions
- Phase 00 baseline green.
- `pinact` or `frizbee` installed locally (for SHA-pin generation).

## Files to touch
- `.github/workflows/release.yml`
- `.github/workflows/pr.yml`
- `.github/workflows/test.yml`
- `.github/workflows/overlay-ci.yml` (already SHA-pinned at `:21` — use
  as the reference; extend its shellcheck call).
- (no source code changes.)

## Steps

### 6.1 — Install pinact
```bash
# https://github.com/suzuki-shunsuke/pinact
go install github.com/suzuki-shunsuke/pinact/v3/cmd/pinact@latest
# OR via aqua/asdf — same outcome
```

### 6.2 — Pin every action
Run `pinact run` against each workflow. Review the diff manually — the
tool rewrites `uses: foo/bar@v4` to `uses: foo/bar@<sha> # v4`. Confirm
the `# v4` comment is preserved (so future updates remain readable).

Specific lines to verify after the run (audit02 P0-H references HEAD `554255d`):

| File:line                   | Before                       | After                                 |
|-----------------------------|------------------------------|---------------------------------------|
| `release.yml:32`            | `actions/checkout@v4`        | `actions/checkout@<sha> # v4.2.2`     |
| `release.yml:38`            | `dtolnay/rust-toolchain@stable` | `dtolnay/rust-toolchain@<sha> # stable` |
| `release.yml:42`            | `Swatinem/rust-cache@v2`     | `Swatinem/rust-cache@<sha> # v2.7.5`  |
| `pr.yml:34`, `test.yml:14`  | `Swatinem/rust-cache@v2`     | (same SHA as above)                   |

For `dtolnay/rust-toolchain@stable`, also add a `rust-toolchain.toml` at
repo root:
```toml
[toolchain]
channel = "1.85.0"  # whatever stable was used to publish; bump explicitly
components = ["clippy", "rustfmt"]
```
Then `dtolnay/rust-toolchain` reads the channel from that file
deterministically.

### 6.3 — Move `permissions:` from workflow-level to job-level (P1-O)
`release.yml` currently has `permissions: contents: write` at the top
(line 7), inherited by all three jobs. Move to per-job:
```yaml
# release.yml
on:
  release:
    types: [created]

# REMOVE the workflow-level `permissions:` block.

jobs:
  build:
    runs-on: ${{ matrix.os }}
    permissions:
      contents: write    # upload assets
    # ...
  upload-install-script:
    runs-on: ubuntu-24.04
    permissions:
      contents: write    # upload install.sh
    # ...
  update-release-notes:
    runs-on: ubuntu-24.04
    permissions:
      contents: write    # edit release body
    # ...
```
Future signing job will add `id-token: write` to its own job only.

### 6.4 — Add explicit `permissions:` to PR and test workflows (P1-R)
At the top of `pr.yml` and `test.yml`:
```yaml
permissions:
  contents: read
```
Default-deny — fork PRs cannot escalate.

### 6.5 — Cache hardening (P1-N)
For `Swatinem/rust-cache@<sha>` in `pr.yml:34` and `test.yml:14`, add:
```yaml
- uses: Swatinem/rust-cache@<sha> # v2.7.5
  with:
    save-if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' }}
    cache-on-failure: false
```
Untrusted PR forks can't write to the cache; only `main` push events
populate it.

### 6.6 — Lint top-level install.sh + bashrc-additions.sh (P1-T)
Edit `overlay-ci.yml:29`:
```yaml
- run: |
    shellcheck \
      install.sh \
      dux-amq/install.sh \
      dux-amq/wrappers/* \
      dux-amq/scripts/*.sh \
      dux-amq/config/bashrc-additions.sh
```
The `bashrc-additions.sh` file contains a runtime `eval` (line ~30) and
must be linted.

### 6.7 — Add `concurrency:` block to release (P2)
```yaml
# release.yml top-level
concurrency:
  group: release-${{ github.event.release.tag_name }}
  cancel-in-progress: false
```

## Validation
- `pinact run --check` reports 0 unpinned references.
- `gh pr checks` green on the PR.
- View any past green CI run to confirm action SHAs match what `pinact`
  generated (sanity).
- `shellcheck -V` ≥ 0.9; running it locally on the new file list passes.

## Acceptance criteria
- [ ] All `uses: foo/bar@v...` lines in release/pr/test workflows pinned
      to commit SHA with `# vX.Y.Z` comment.
- [ ] `rust-toolchain.toml` at repo root.
- [ ] `release.yml` has no workflow-level `permissions:`; each job sets its own.
- [ ] `pr.yml` and `test.yml` have `permissions: { contents: read }`.
- [ ] `Swatinem/rust-cache` has `save-if` + `cache-on-failure: false`.
- [ ] `overlay-ci.yml` shellcheck covers the top-level install.sh and
      bashrc-additions.sh.
- [ ] `release.yml` has `concurrency:` block.
- [ ] PR: `ci(security): pin actions by SHA, tighten permissions (P0-H, P1-N/O/R/T)`.

## Known pitfalls
- `pinact` may rewrite `# Comment` lines next to `uses:` — review and
  restore. Use `--ignore-path` for any reusable workflow you can't pin
  (rare; we have none).
- `dtolnay/rust-toolchain` SHA-pinning + `rust-toolchain.toml` is the
  right combo, but if you forget the toolchain file the action falls
  back to `stable` from the SHA-pinned commit at the time of pinning,
  which can drift versus your current `cargo --version`.
- `concurrency: cancel-in-progress: false` for releases is intentional
  — never cancel an in-flight release upload.
- After pinning, dependabot config (Phase 24) is what keeps the SHAs
  fresh; without it pinning becomes a stale-deps liability.

## References
- audit02 P0-H, P1-N, P1-O, P1-R, P1-T.
- StepSecurity: Pinning GitHub Actions guide.
- pinact: https://github.com/suzuki-shunsuke/pinact
- GitHub Changelog (Aug 2025): SHA-pinning policy support.
