//! Placeholder for sub-project #3 — the web layer that will expose the
//! `dux-core` engine over HTTP/WebSocket. Empty for now so the workspace
//! topology is ready: this crate depends on `dux-core` (not `dux-tui`).
//!
//! Dependency isolation is enforced by the `dep-isolation` CI job, which
//! runs `cargo tree -p dux-web` and fails if any TUI-only crate appears.

#[cfg(test)]
mod tests {
    use dux_core::engine::Command;

    /// Light smoke test that the public dux-core API can be invoked from
    /// dux-web without TUI imports. Real architectural enforcement of the
    /// "no TUI deps" rule lives in the `dep-isolation` CI job.
    #[test]
    fn dux_core_command_is_constructible() {
        let cmd = Command::OpenPath {
            path: std::path::PathBuf::from("/tmp/dux-web-smoke"),
            target: "session worktree".to_string(),
        };
        // Exercise pattern-matching so the variant fields are actually
        // referenced — a dead-code construction wouldn't catch API drift.
        match cmd {
            Command::OpenPath { path, target } => {
                assert_eq!(target, "session worktree");
                assert_eq!(path.display().to_string(), "/tmp/dux-web-smoke");
            }
            _ => unreachable!("constructed an OpenPath variant"),
        }
    }
}
