//! Placeholder for sub-project #3 — the web layer that will expose the
//! `dux-core` engine over HTTP/WebSocket. Empty for now so the workspace
//! topology is ready: this crate depends on `dux-core` (not `dux-tui`),
//! proving the headless engine is reusable.

#[cfg(test)]
mod tests {
    /// Compile-time check that `dux-web` can construct types from
    /// `dux-core` without dragging in any TUI deps. If sub-project #3
    /// (or a future architectural drift) accidentally pulls `dux-tui`
    /// into `dux-web`'s dependency graph, this test would still pass —
    /// the real invariant is enforced by `cargo tree` checks in CI /
    /// the E5 plan. This test exists so the crate has at least one
    /// piece of executable code proving the dep wiring works.
    #[test]
    fn dux_core_types_are_reachable() {
        // Engine has a public constructor only through `bootstrap_with_lock`
        // which needs paths + a lock; we don't construct one here. Just
        // referencing the path proves the symbol is reachable from this
        // crate without any TUI imports.
        let _ = std::any::TypeId::of::<dux_core::engine::Engine>();
        let _ = std::any::TypeId::of::<dux_core::engine::Command>();
        let _ = std::any::TypeId::of::<dux_core::engine::EventReaction>();
    }
}
