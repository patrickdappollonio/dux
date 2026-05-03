# Phase 03: Sanitizer module — log injection + status-line stderr

> Maps to: **P0-B** (logger.rs raw bytes), **P0-C** (status line carries unsanitized stderr).

## Goal
Stop attacker-controlled bytes (git stderr, PR titles from `gh pr view`,
`/proc/<pid>/comm`, branch names) from reaching `dux.log` or the status
line as raw ANSI/OSC/DCS sequences. A single `sanitize_for_terminal`
helper, called from both `logger::*` and the `set_error/set_info` shims.

## Pre-conditions
- Phase 00 baseline green.
- Independent of Phases 01, 02, 04, 05.

## Files to touch
- `src/sanitize.rs` — NEW module.
- `src/main.rs` (or `src/lib.rs` if present) — `mod sanitize;`.
- `src/logger.rs` — call sanitizer in `log()`.
- `src/app/mod.rs` — call sanitizer in `set_error`/`set_info` (whichever
  shim builds the displayed string).
- `src/git.rs` — switch `String::from_utf8_lossy(&output.stderr)` to
  `sanitize::utf8_lossy(&output.stderr)`.
- `tests/sanitize.rs` — NEW integration test (also unit tests inline).

## Steps

### 3.1 — Write the sanitizer
`src/sanitize.rs`:
```rust
//! Strip ANSI/OSC/DCS/control bytes from operator-visible strings.
//!
//! Operator-trust strings (log lines, status messages, error popups) MUST
//! pass through this filter. Without it, an attacker who controls a git
//! stderr message, PR title, branch name, or process name can inject
//! escape sequences that rewrite the operator's terminal title (OSC 0/2),
//! drop covering OSC 8 hyperlinks, or paste-inject via OSC 52 the next
//! time `tail dux.log` is run. Same class as Rails CVE-2025-55193.

const SAFE_NEWLINE: char = '\n';
const SAFE_TAB: char = '\t';

/// Strip control bytes and ESC; preserve printable + `\t` + `\n`.
/// Replaces stripped bytes with their `\xNN` hex form so operators can
/// still see what was filtered (no silent loss).
pub fn for_terminal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            SAFE_NEWLINE | SAFE_TAB => out.push(c),
            c if c.is_control() => {
                use std::fmt::Write;
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c if (c as u32) == 0x7f => out.push_str("\\x7f"),
            c if (c as u32) == 0x9b => out.push_str("\\x9b"), // CSI 8-bit
            c => out.push(c),
        }
    }
    out
}

/// Convenience: like `String::from_utf8_lossy(...).to_string()` but also
/// runs `for_terminal`. Use for command stderr where bytes are bounded.
pub fn utf8_lossy(bytes: &[u8]) -> String {
    for_terminal(&String::from_utf8_lossy(bytes))
}

/// Truncate after sanitization so `\xNN` expansions don't overflow.
pub fn truncate(s: &str, max_chars: usize) -> String {
    let cleaned = for_terminal(s);
    if cleaned.chars().count() <= max_chars {
        cleaned
    } else {
        cleaned.chars().take(max_chars - 1).chain(std::iter::once('…')).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_osc_title_set() {
        let s = "\x1b]2;rm -rf $HOME\x07";
        let out = for_terminal(s);
        assert!(!out.contains('\x1b'));
        assert!(!out.contains('\x07'));
        assert!(out.contains("\\x1b"));
    }

    #[test]
    fn preserves_newlines_and_tabs() {
        assert_eq!(for_terminal("a\tb\nc"), "a\tb\nc");
    }

    #[test]
    fn handles_8bit_csi() {
        assert!(for_terminal("\u{009b}A").contains("\\x9b"));
    }

    #[test]
    fn utf8_lossy_handles_invalid_bytes() {
        let bytes = b"hello \xff\x1b]2;evil\x07 world";
        let out = utf8_lossy(bytes);
        assert!(!out.contains('\x1b'));
        assert!(out.contains("hello"));
        assert!(out.contains("world"));
    }

    #[test]
    fn truncate_with_ellipsis() {
        let s = "0123456789";
        assert_eq!(truncate(s, 5), "0123…");
    }
}
```

### 3.2 — Wire into logger.rs
`src/logger.rs:84-92` — change to:
```rust
let line = format!(
    "{} {:<5} {}\n",
    Utc::now().to_rfc3339(),
    level.as_str(),
    crate::sanitize::for_terminal(message),  // <-- new
);
```
Add `use crate::sanitize;` at top.

### 3.3 — Wire into status-line shims
Find `set_error`/`set_info` in `src/app/mod.rs` (grep `fn set_error\|fn set_info`).
The signatures take `String`/`&str` and build the `Statusline` payload.
Wrap once:
```rust
pub(crate) fn set_error(&mut self, msg: impl Into<String>) {
    let cleaned = crate::sanitize::truncate(&msg.into(), 512);
    // … existing logic, but with `cleaned` instead of the raw string.
}
```
Same for `set_info`. **Do not** push the sanitizer into every call site;
single point of entry is the goal.

### 3.4 — Switch git.rs to `utf8_lossy`
`src/git.rs` — in every `Err(anyhow!(... String::from_utf8_lossy(&output.stderr) ...))`
(approx 17 sites listed in audit02 P0-C), replace with:
```rust
Err(anyhow!("git foo failed: {}", crate::sanitize::utf8_lossy(&output.stderr)))
```
Use `replace_all` semantics carefully — verify each site visually.

### 3.5 — Test
`tests/sanitize.rs`:
```rust
use dux::sanitize;  // exposed via lib.rs / pub use

#[test]
fn end_to_end_log_line_has_no_escapes() {
    let evil_branch = "feat-\x1b]2;evil\x07-x";
    let cleaned = sanitize::for_terminal(evil_branch);
    assert!(!cleaned.contains('\x1b'));
    assert!(!cleaned.contains('\x07'));
}
```

## Validation
- `cargo test sanitize` green.
- `cargo test --test sanitize` integration test green.
- Manual: in a test repo, `git checkout -b $'feat-\e]2;evil\a-x'`; run
  `dux`; trigger an error; `cat dux.log` — should show `\x1b]2;evil\x07`
  literally, not interpret it.

## Acceptance criteria
- [ ] `src/sanitize.rs` exists with 4 unit tests passing.
- [ ] `logger.rs::log` runs `sanitize::for_terminal(message)`.
- [ ] `set_error`/`set_info` run `sanitize::truncate(..., N)`.
- [ ] All `String::from_utf8_lossy(&output.stderr)` in `src/git.rs`
      replaced with `sanitize::utf8_lossy`.
- [ ] Integration test asserts no escape bytes survive.
- [ ] `cargo clippy --all-targets -- -D warnings` green.
- [ ] PR: `feat(security): sanitize operator-visible strings (P0-B/C)`.

## Known pitfalls
- The sanitizer is **called from inside `logger::log`**, which means
  the sanitizer itself must NOT log on any code path — would loop.
  Tests guarantee this by avoiding `logger::*` calls; reviewers must
  also check.
- Status messages may already be UTF-16-ish via `compact_str` — confirm
  the `set_error` parameter type. If it's `Cow<str>`, the same approach
  works.
- Don't sanitize PTY render output — that's the alacritty terminal grid,
  which legitimately needs escape interpretation. The sanitizer is only
  for *operator-trust* strings (log + status + diff captions), not for
  arbitrary terminal child output.

## References
- audit02 P0-B, P0-C.
- Rails ActiveRecord ANSI log injection (CVE-2025-55193) — same fix shape.
- dgl.cx: ANSI terminal security (2023).
