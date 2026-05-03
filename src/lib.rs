//! `dux` library entry point.
//!
//! This crate is primarily a binary (`src/main.rs`) but a thin library
//! interface is exposed here so integration tests under `tests/` can drive
//! internal types such as [`storage::SessionStore`] without resorting to
//! `#[path]` workarounds.
//!
//! Both crates compile the same module sources; only `main.rs` wires them
//! into the running TUI. Library consumers are expected to be tests, not
//! external crates — there is no API stability guarantee.

pub mod app;
pub mod cli;
pub mod clipboard;
pub mod config;
pub mod diff;
pub mod editor;
pub mod git;
pub mod io_retry;
pub mod keybindings;
pub mod lockfile;
pub mod logger;
pub mod model;
pub mod provider;
pub mod pty;
pub mod raw_input;
pub mod sanitize;
pub mod statusline;
pub mod storage;
pub mod theme;
