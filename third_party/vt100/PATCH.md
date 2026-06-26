# Vendored `vt100` 0.16.2 (dux patch)

This directory is a **verbatim copy of `vt100` 0.16.2 from crates.io** with
exactly **one behavioral change**. The workspace root `Cargo.toml` redirects the
`vt100` dependency here via:

```toml
[patch.crates-io]
vt100 = { path = "third_party/vt100" }
```

Keep the tree byte-faithful to upstream except for the single patch described
below. Do not reformat it to dux's `rustfmt` style — it is upstream code.

## The change

In `src/grid.rs`, `Grid::scroll_up()`, the guard that decides whether an evicted
line is pushed into scrollback:

```rust
// upstream
if self.scrollback_len > 0 && !self.scroll_region_active() {

// dux patch
if self.scrollback_len > 0 && self.scroll_top == 0 {
```

`scroll_region_active()` is `self.scroll_top != 0 || self.scroll_bottom != self.size.rows - 1`.

## Why

Agent CLIs (codex, claude, …) pin a status/input bar at the bottom of the screen
by installing a DECSTBM scroll region with a bottom margin (e.g. `ESC[1;4r`).
That makes `scroll_region_active()` always true, so upstream `vt100` **discards**
every transcript line that scrolls off the top of the region. The result inside
dux: `history_len()` stays `0` and PgUp / PgDn / mouse-wheel scrollback is dead
for exactly the tools dux exists to drive.

The correct, standard behavior (used by **xterm, alacritty, and wezterm**) is to
capture scrolled-off lines whenever the scroll region is **top-anchored**
(`scroll_top == 0`), regardless of the bottom margin. The patch does exactly
that. A region with a non-zero top (a true split-screen window) still bypasses
scrollback, matching upstream intent.

`scroll_region_active()` becomes unused after the patch but is intentionally
**kept** (with `#[allow(dead_code)]`) so the diff against upstream stays minimal
and obvious. Do not delete it.

## Re-applying on a `vt100` version bump

1. Re-vendor the new upstream source:
   ```bash
   rm -rf third_party/vt100
   cp -R "$CARGO_HOME/registry/src/index.crates.io-*/vt100-<NEW_VERSION>" third_party/vt100
   chmod -R u+w third_party/vt100
   rm -f third_party/vt100/.cargo-ok third_party/vt100/.cargo_vcs_info.json third_party/vt100/Cargo.toml.orig
   ```
   Keep the `LICENSE*` files (vt100 is MIT — the license must stay).
2. Confirm `third_party/vt100/Cargo.toml` `version` matches what dux requests in
   `crates/dux-core/Cargo.toml` (so the `[patch.crates-io]` redirect resolves).
3. Re-apply the one-liner in `src/grid.rs`'s `scroll_up`: change the upstream
   `!self.scroll_region_active()` guard to `self.scroll_top == 0`, and restore
   the explanatory comment.
4. Re-add `#[allow(dead_code)]` above `fn scroll_region_active` if it is still
   otherwise unused.
5. Restore this `PATCH.md`.
6. Run `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
   and `cargo test` — the regression test
   `scroll_region_with_bottom_margin_still_captures_scrollback` in
   `crates/dux-core/src/pty.rs` guards this behavior.
