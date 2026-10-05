//! dux-core: the headless domain layer for dux.
//!
//! This crate must not depend on `ratatui`, `crossterm`, or any web/server
//! crate. Surfaces (TUI, web) depend on `dux-core`, never the reverse.

pub mod action;
pub mod activity;
pub mod add_project_plan;
pub mod add_project_prose;
pub mod agent_job;
pub mod agent_search;
pub mod agent_tabs;
pub mod attention;
pub mod auth;
pub mod background_serve;
pub mod base_branch;
pub mod bidi;
pub mod bounded_command;
pub mod browser;
pub mod changes_status;
pub mod config;
pub mod config_auth;
pub mod config_effective;
pub mod config_keys;
pub mod config_migrate;
pub mod config_queue;
pub mod config_reload_status;
pub mod config_sync;
pub mod config_write;
pub mod container;
pub mod device_label;
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
pub mod ids;
pub mod io_retry;
pub mod lockfile;
pub mod logger;
pub mod macros;
pub mod model;
pub mod palette;
pub mod poller_status;
pub mod pr_reference;
pub mod project_browser;
pub mod project_order;
pub mod project_prose;
pub mod prose;
pub mod provider;
pub mod pty;
pub mod pty_owners;
pub mod qr;
pub mod quiet_tail;
pub mod release_notes;
pub mod reload_signal;
pub mod resource_stats;
pub mod row_state;
pub mod scroll_hint;
pub mod scroll_margins;
pub mod serve_log;
pub mod shell_quote;
pub mod sidebar;
#[cfg(any(test, feature = "test-support"))]
pub mod start_check_fixtures;
pub mod startup;
pub mod status_text;
pub mod statusline;
pub mod storage;
pub mod tab_verdict;
pub mod tailscale;
pub mod term_identity;
pub mod terminal_title;
#[cfg(any(test, feature = "test-support"))]
pub mod test_git;
#[cfg(any(test, feature = "test-support"))]
pub mod test_provider;
#[cfg(any(test, feature = "test-support"))]
pub mod test_scratch;
pub mod text;
pub mod theme;
pub mod urls;
pub mod viewmodel;
pub mod web_sessions;
pub mod welcome;
pub mod welcome_screen;
pub mod wire;
pub mod worker;
pub mod working_copy;
pub mod working_cue;
pub mod worktree_file;
pub mod worktree_manager;

/// Display version string ('vX.Y.Z' for release builds, 'development' otherwise), set by build.rs, mirroring the TUI's `DUX_DISPLAY_VERSION`.
pub fn display_version() -> &'static str {
    env!("DUX_DISPLAY_VERSION")
}
