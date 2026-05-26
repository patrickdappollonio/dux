//! dux-core: the headless domain layer for dux.
//!
//! This crate must not depend on `ratatui`, `crossterm`, or any web/server
//! crate. Surfaces (TUI, web) depend on `dux-core`, never the reverse.

pub mod browser;
pub mod editor;
pub mod io_retry;
pub mod model;
pub mod statusline;
pub mod theme;
