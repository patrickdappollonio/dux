//! Surface-agnostic theme identity.
//!
//! The full theme model — the ratatui `Theme` struct, opaline-backed color
//! loading, and `Style`/`Span` helpers — is rendering-specific and lives in the
//! TUI surface (its loaders also depend on config/logger). Only theme identity
//! that domain/config code needs belongs here.

/// Name of the bundled default theme — also the value written into the
/// generated `config.toml` on first boot.
pub const DEFAULT_THEME_NAME: &str = "dux_dark";
