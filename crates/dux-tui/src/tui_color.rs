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
    for (attribute, modifier) in [
        (CellModifier::BOLD, Modifier::BOLD),
        (CellModifier::DIM, Modifier::DIM),
        (CellModifier::ITALIC, Modifier::ITALIC),
        (CellModifier::UNDERLINED, Modifier::UNDERLINED),
        (CellModifier::REVERSED, Modifier::REVERSED),
        (CellModifier::CROSSED_OUT, Modifier::CROSSED_OUT),
    ] {
        if m.contains(attribute) {
            out.insert(modifier);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_cell_attribute_becomes_its_ratatui_modifier() {
        let pairs = [
            (CellModifier::BOLD, Modifier::BOLD),
            (CellModifier::DIM, Modifier::DIM),
            (CellModifier::ITALIC, Modifier::ITALIC),
            (CellModifier::UNDERLINED, Modifier::UNDERLINED),
            (CellModifier::REVERSED, Modifier::REVERSED),
            (CellModifier::CROSSED_OUT, Modifier::CROSSED_OUT),
        ];
        for (cell, expected) in pairs {
            assert_eq!(to_ratatui_modifier(cell), expected);
        }
        assert_eq!(
            to_ratatui_modifier(CellModifier::default()),
            Modifier::empty()
        );
    }
}
