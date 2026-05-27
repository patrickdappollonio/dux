use dux_core::pty::{CellColor, CellModifier};
use ratatui::style::{Color, Modifier};

pub(crate) fn to_ratatui_color(c: CellColor) -> Color {
    match c {
        CellColor::Reset => Color::Reset,
        CellColor::Black => Color::Black,
        CellColor::Red => Color::Red,
        CellColor::Green => Color::Green,
        CellColor::Yellow => Color::Yellow,
        CellColor::Blue => Color::Blue,
        CellColor::Magenta => Color::Magenta,
        CellColor::Cyan => Color::Cyan,
        CellColor::Gray => Color::Gray,
        CellColor::DarkGray => Color::DarkGray,
        CellColor::LightRed => Color::LightRed,
        CellColor::LightGreen => Color::LightGreen,
        CellColor::LightYellow => Color::LightYellow,
        CellColor::LightBlue => Color::LightBlue,
        CellColor::LightMagenta => Color::LightMagenta,
        CellColor::LightCyan => Color::LightCyan,
        CellColor::White => Color::White,
        CellColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
        CellColor::Indexed(i) => Color::Indexed(i),
    }
}

pub(crate) fn to_ratatui_modifier(m: CellModifier) -> Modifier {
    let mut out = Modifier::empty();
    if m.bold { out.insert(Modifier::BOLD); }
    if m.dim { out.insert(Modifier::DIM); }
    if m.italic { out.insert(Modifier::ITALIC); }
    if m.underlined { out.insert(Modifier::UNDERLINED); }
    if m.reversed { out.insert(Modifier::REVERSED); }
    if m.crossed_out { out.insert(Modifier::CROSSED_OUT); }
    out
}
