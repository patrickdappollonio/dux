//! Surface-agnostic terminal cell color and style, produced by the PTY
//! terminal-grid snapshot. Each surface converts these to its own medium
//! (the TUI to `ratatui` types; the web to CSS) at its render boundary.

/// Mirrors the variant set of `ratatui::style::Color` so the PTY snapshot can
/// describe any cell color without depending on a UI toolkit. The TUI converts
/// 1:1 to `ratatui::Color`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellColor {
    Reset,
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    Gray,
    DarkGray,
    LightRed,
    LightGreen,
    LightYellow,
    LightBlue,
    LightMagenta,
    LightCyan,
    White,
    Rgb(u8, u8, u8),
    Indexed(u8),
}

/// The subset of text attributes the terminal grid carries. Mirrors the
/// `ratatui::style::Modifier` flags the snapshot sets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellModifier {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underlined: bool,
    pub reversed: bool,
    pub crossed_out: bool,
}
