# Phase 21: macOS CI matrix + OsStr-based git invocation portability

> Maps to: **P1-S**, **P2-11** (audit02).

## Goal
Two things:
1. Add macOS to PR/test CI matrices. `release.yml` ships macOS
   binaries but CI never tests on macOS — every macOS bug is found by
   downstream users.
2. Replace `path.to_string_lossy()` in git invocations with `OsStr`
   passing so non-UTF-8 worktree paths work on macOS HFS+/APFS.

## Pre-conditions
- Phase 06 (GHA pinning) merged — every new action added here must
  also be SHA-pinned.
- Phase 04 (UI thread workers) merged — no API churn during this work.

## Files to touch
- `.github/workflows/pr.yml` — add `os: [ubuntu-latest, macos-latest]`.
- `.github/workflows/test.yml` — same.
- `.github/workflows/release.yml` — pin `macos-13` (Intel) + `macos-14`
  (ARM) explicitly (P2-5).
- `src/git.rs` — switch arg passing from `to_string_lossy()` to `OsStr`.
- `tests/git_portability.rs` — NEW.

## Steps

### 21.1 — CI matrix
`pr.yml` and `test.yml` build/clippy/test job:
```yaml
strategy:
  fail-fast: false
  matrix:
    os: [ubuntu-24.04, macos-14]
runs-on: ${{ matrix.os }}
steps:
  - uses: actions/checkout@<sha>
  - uses: dtolnay/rust-toolchain@<sha>
  - uses: Swatinem/rust-cache@<sha>
    with:
      save-if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' }}
  - run: cargo fmt --check
  - run: cargo clippy --all-targets --all-features -- -D warnings
  - run: cargo test --all-features
```

For `release.yml`:
```yaml
matrix:
  include:
    - os: ubuntu-24.04
      target: x86_64-unknown-linux-gnu
    - os: ubuntu-24.04
      target: aarch64-unknown-linux-gnu  # cross-compile
    - os: macos-13
      target: x86_64-apple-darwin
    - os: macos-14
      target: aarch64-apple-darwin
```

### 21.2 — OsStr-based git invocation
`src/git.rs` — every place that does:
```rust
Command::new("git")
    .arg("-C")
    .arg(path.to_string_lossy().into_owned())
    .arg("status")
```
becomes:
```rust
Command::new("git")
    .arg("-C")
    .arg(path.as_os_str())   // or simply .arg(path) — Path: AsRef<OsStr>
    .arg("status")
```
This preserves non-UTF-8 bytes verbatim across `Command::arg`. Audit
every `.to_string_lossy()` in `src/git.rs` (audit02 P2-11 estimates
~15 sites).

### 21.3 — Tests
`tests/git_portability.rs`:
```rust
#[test]
#[cfg(unix)]
fn git_handles_non_utf8_worktree_path() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let tmp = tempfile::tempdir().unwrap();
    let bad = tmp.path().join(OsString::from_vec(vec![0xff, 0xfe, b'd', b'i', b'r']));
    std::fs::create_dir(&bad).unwrap();
    // Init a git repo there
    std::process::Command::new("git").arg("init").current_dir(&bad).output().unwrap();
    // Our wrapper should not panic and should return a sensible result.
    let result = dux::git::is_git_repo(&bad);
    assert!(result);
}
```

### 21.4 — macOS-specific quirks
- `realpath` on macOS lacks `--`; use `realpath PATH` (no double dash)
  or rely on `coreutils`'s `grealpath`. Phase 12 fix already needs
  attention here.
- `flock` (Phase 11) is util-linux only; macOS migration phase is
  Linux-only per README, so this is fine — but document.
- `pgrep -x` works the same; OK.
- `sed -i` syntax differs (`-i ''` on macOS); avoid in shipped scripts
  or use `sed -i.bak …` portably.

### 21.5 — Update Makefile
```makefile
test-all-platforms:
	@echo "Run via CI matrix; locally use Linux + macOS hosts."
```

## Validation
- `gh pr checks` shows green Linux AND macOS jobs.
- `tests/git_portability.rs` passes on Linux (`#[cfg(unix)]` covers macOS too).
- `cargo build --target x86_64-apple-darwin` succeeds on a macOS runner.

## Acceptance criteria
- [ ] PR/test workflows have macOS in matrix.
- [ ] release.yml pins `macos-13` for Intel, `macos-14` for ARM.
- [ ] All `to_string_lossy()` in `src/git.rs` replaced with `path.as_os_str()`
      or direct `Path` arg passing.
- [ ] Non-UTF-8 portability test passes on Linux (and macOS via CI).
- [ ] PR: `ci(matrix): add macOS + OsStr-portable git args (P1-S, P2-11)`.

## Known pitfalls
- macOS GitHub runner billing is ~10× Linux — keep the matrix lean
  (one OS per job; not a 2x2x2 matrix).
- Some macOS-specific cargo cache quirks; `Swatinem/rust-cache` handles
  but expect first-run slowness.
- Cross-compile from Linux to aarch64 requires `gcc-aarch64-linux-gnu`;
  install via `apt-get install gcc-aarch64-linux-gnu` in matrix step.
- macOS `clipboard.rs` uses `arboard` which talks to `NSPasteboard`;
  if a CI macOS runner has no GUI session, clipboard tests may need
  to be `#[cfg(not(macos))]` or feature-gated.

## References
- audit02 P1-S, P2-11.
