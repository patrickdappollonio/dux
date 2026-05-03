//! Claude Code on-disk project-dir encoder (Rust port).
//!
//! Single source of truth used by:
//!   - `crate::purge` — the GDPR hard-purge command (audit02 Phase 10).
//!   - the eventual Phase 12 wrapper-side encoder fix; this module is
//!     intentionally written so that `dux-amq/scripts/encode-claude-project-dir`
//!     and `crate::purge_encoding::encode_claude_project_dir` produce
//!     byte-identical output. A drift would orphan provider chat dirs.
//!
//! ## Encoding rules
//!
//! Verified against Claude Code 2.1.111 on Linux. See
//! `dux-amq/scripts/encode-claude-project-dir` for the rule derivation
//! transcript and `dux-amq/tests/fixtures/claude-paths.txt` for the
//! reference fixture used by the unit tests below.
//!
//!   1. Strip a single trailing `/` if present (preserve `"/"` itself).
//!   2. Replace every character that is NOT in `[A-Za-z0-9-]` with `-`.
//!      This single rule covers `/`, `_`, `.`, space, parens, `@`, `+`,
//!      `:` and every other separator we probed. Runs of unsafe chars
//!      are NOT collapsed: `__` becomes `--`, not `-`.
//!   3. Case is preserved (`Foo_Bar` → `Foo-Bar`).
//!
//! ## Why a Rust port
//!
//! Phase 10 (GDPR purge) needs the encoder on every code path that
//! decides which provider chat dir belongs to a session. Shelling out
//! to the bash script per session would force a process spawn per
//! purge target, complicate error-handling, and tie test runs to the
//! presence of `dux-amq/scripts/` on disk. The Rust implementation is
//! pure and trivially testable.
//!
//! ## Invariants
//!
//! - `encode_claude_project_dir` is total — every absolute path produces
//!   *some* string. Empty input returns `Err`.
//! - Output contains only ASCII bytes from `[A-Za-z0-9-]`. This is
//!   filesystem-safe on every platform dux targets.
//! - The bash script and this module MUST stay in sync. The fixture
//!   test below loads the same `claude-paths.txt` the bash test consumes
//!   and asserts byte-for-byte equivalence.

use std::path::Path;

/// Encode an absolute filesystem path the way Claude Code names its
/// on-disk session dirs at `~/.claude/projects/<encoded>`.
///
/// Returns an error if the input is empty or relative — Claude Code
/// itself only ever stores absolute paths, and a relative input would
/// silently produce a wrong directory name.
#[allow(dead_code)] // `crate::purge` uses `encode_str`; this is the `Path`-typed convenience for future call sites and integration tests.
pub fn encode_claude_project_dir(path: &Path) -> anyhow::Result<String> {
    let raw = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("path is not valid UTF-8: {}", path.display()))?;
    encode_str(raw)
}

/// String-input variant used by tests and call sites that already have
/// the path as a UTF-8 string. Same rules as `encode_claude_project_dir`.
pub fn encode_str(raw: &str) -> anyhow::Result<String> {
    if raw.is_empty() {
        anyhow::bail!("empty path is not encodable");
    }
    if !raw.starts_with('/') {
        anyhow::bail!("absolute path required, got {raw:?}");
    }

    // Step 1: strip a single trailing `/` (but preserve `"/"` itself —
    // stripping would yield "" and we then have nothing to encode).
    let trimmed: &str = if raw == "/" {
        raw
    } else {
        raw.strip_suffix('/').unwrap_or(raw)
    };

    // Step 2: replace every char outside `[A-Za-z0-9-]` with `-`.
    // Runs are NOT collapsed — `__` becomes `--`, matching the bash
    // script's behaviour.
    let mut out = String::with_capacity(trimmed.len());
    for ch in trimmed.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' {
            out.push(ch);
        } else {
            out.push('-');
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Load the canonical fixture used by both the bash script's bats
    /// test and this Rust unit test. Lines starting with `#` and blank
    /// lines are skipped; non-comment lines are TAB-separated
    /// `<input>\t<expected>` records.
    fn load_fixtures() -> Vec<(String, String)> {
        // Repo-relative path; tests run from the crate root.
        let path = std::path::Path::new("dux-amq/tests/fixtures/claude-paths.txt");
        let raw = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("failed to read fixture {}: {e}", path.display()));
        let mut out = Vec::new();
        for line in raw.lines() {
            let line = line.trim_end_matches('\r');
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let mut parts = line.splitn(2, '\t');
            let input = parts.next().expect("input column");
            let expected = parts.next().unwrap_or_else(|| {
                panic!("fixture line missing TAB separator: {line:?}");
            });
            out.push((input.to_string(), expected.to_string()));
        }
        assert!(
            out.len() >= 6,
            "claude-paths.txt should have at least 6 cases (audit02 Phase 12 acceptance)"
        );
        out
    }

    #[test]
    fn matches_canonical_fixtures() {
        for (input, expected) in load_fixtures() {
            let got =
                encode_str(&input).unwrap_or_else(|e| panic!("encode_str({input:?}) failed: {e}"));
            assert_eq!(got, expected, "fixture mismatch for input {input:?}");
        }
    }

    #[test]
    fn rejects_relative_paths() {
        assert!(encode_str("relative/path").is_err());
        assert!(encode_str("foo").is_err());
    }

    #[test]
    fn rejects_empty_input() {
        assert!(encode_str("").is_err());
    }

    #[test]
    fn root_path_preserved_as_dash() {
        // "/" → "-" (one char outside the safe class).
        assert_eq!(encode_str("/").unwrap(), "-");
    }

    #[test]
    fn trailing_slash_stripped_then_encoded() {
        assert_eq!(
            encode_str("/tmp/probe/trailing/").unwrap(),
            "-tmp-probe-trailing"
        );
    }

    #[test]
    fn case_preserved() {
        assert_eq!(encode_str("/foo/MixedCase").unwrap(), "-foo-MixedCase");
    }

    #[test]
    fn runs_of_unsafe_chars_not_collapsed() {
        // `__` -> `--`, not `-`.
        assert_eq!(
            encode_str("/foo/double__under").unwrap(),
            "-foo-double--under"
        );
        // `..` -> `--`.
        assert_eq!(
            encode_str("/foo/with..dotdot").unwrap(),
            "-foo-with--dotdot"
        );
    }

    #[test]
    fn path_input_variant_works() {
        let p = std::path::Path::new("/tmp/probe/with-dash");
        assert_eq!(
            encode_claude_project_dir(p).unwrap(),
            "-tmp-probe-with-dash"
        );
    }
}
