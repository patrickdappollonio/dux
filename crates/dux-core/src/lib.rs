//! dux-core: the headless domain layer for dux.
//!
//! This crate must not depend on `ratatui`, `crossterm`, or any web/server
//! crate. Surfaces (TUI, web) depend on `dux-core`, never the reverse.

pub mod action;
pub mod agent_job;
pub mod auth;
pub mod browser;
pub mod config;
pub mod config_write;
pub mod diff;
pub mod editor;
pub mod engine;
pub mod gh;
pub mod git;
pub mod io_retry;
pub mod lockfile;
pub mod logger;
pub mod model;
pub mod palette;
pub mod project_browser;
pub mod provider;
pub mod pty;
pub mod resource_stats;
pub mod startup;
pub mod statusline;
pub mod storage;
pub mod theme;
pub mod viewmodel;
pub mod welcome;
pub mod wire;
pub mod worker;
