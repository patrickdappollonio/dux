//! dux-core: the headless domain layer for dux.
//!
//! This crate must not depend on `ratatui`, `crossterm`, or any web/server
//! crate. Surfaces (TUI, web) depend on `dux-core`, never the reverse.

pub mod action;
pub mod activity;
pub mod add_project_plan;
pub mod agent_job;
pub mod agent_search;
pub mod agent_tabs;
pub mod attention;
pub mod background_serve;
pub mod bounded_command;
pub mod browser;
pub mod config;
pub mod config_migrate;
pub mod config_queue;
pub mod config_sync;
pub mod config_write;
pub mod diff;
pub mod editor;
pub mod engine;
pub mod file_drop;
pub mod file_modes;
pub mod first_load;
pub mod flat_list;
pub mod focus;
pub mod gh;
pub mod git;
pub mod gitignore_seed;
pub mod home_path;
pub mod io_retry;
pub mod lockfile;
pub mod logger;
pub mod macros;
pub mod model;
pub mod palette;
pub mod pr_reference;
pub mod project_browser;
pub mod provider;
pub mod pty;
pub mod quiet_tail;
pub mod release_notes;
pub mod resource_stats;
pub mod row_state;
pub mod scroll_hint;
pub mod scroll_margins;
pub mod sidebar;
pub mod startup;
pub mod statusline;
pub mod storage;
pub mod tailscale;
pub mod term_identity;
pub mod terminal_title;
pub mod theme;
pub mod urls;
pub mod viewmodel;
pub mod welcome;
pub mod welcome_screen;
pub mod wire;
pub mod worker;
pub mod worktree_file;
pub mod worktree_manager;

/// Display version string ('vX.Y.Z' for release builds, 'development' otherwise), set by build.rs — mirrors the TUI's `DUX_DISPLAY_VERSION`.
pub fn display_version() -> &'static str {
    env!("DUX_DISPLAY_VERSION")
}
