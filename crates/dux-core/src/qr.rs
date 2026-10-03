//! QR codes for the tailnet addresses `dux server` and the start-web-server flip
//! show, so a phone can open dux without anyone typing a URL.
//!
//! One renderer for both surfaces: `dux server` prints the rows to its console,
//! and the flip's status screen draws the same rows inside its frame. Each code
//! is drawn with Unicode half blocks, two modules per character cell, so a
//! typical tailnet URL fits in about 37 columns and 19 rows.
//!
//! The rows keep the code apart from the words around it ([`Segment`]), because
//! the two surfaces color only the code: a terminal draws it black on white
//! where it can, and the flip's status screen draws it in its theme's colors.

/// Light modules drawn around every code. The QR standard asks for four; two is
/// what phone cameras read comfortably off a screen, and it saves four columns
/// a code, which is the difference between two codes side by side and stacked
/// on an 80-column terminal.
pub const QUIET_ZONE: usize = 2;

/// Columns between two codes drawn side by side, wide enough that a camera
/// framing one does not catch the edge of the other.
pub const COLUMN_GAP: usize = 4;

/// Which modules are drawn as filled blocks.
///
/// A terminal draws a block in its foreground color, so the right choice
/// depends on what is behind the code: on a light background the DARK modules
/// are the blocks, on a dark background the LIGHT ones are, which keeps the
/// code dark-on-light either way, the way the standard and every scanner want it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Polarity {
    /// For a code drawn in a dark foreground on a light background.
    DarkModulesFilled,
    /// For a code drawn in a light foreground on a dark background.
    LightModulesFilled,
}

/// One run of a rendered row: part of a code, or the words and spacing around
/// it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
    Code(String),
    Text(String),
}

impl Segment {
    pub fn text(&self) -> &str {
        match self {
            Segment::Code(text) | Segment::Text(text) => text,
        }
    }
}

/// One rendered row, left to right.
pub type Row = Vec<Segment>;

/// The row as plain text, every segment joined.
pub fn row_text(row: &[Segment]) -> String {
    row.iter().map(Segment::text).collect()
}

/// How many terminal columns the row takes. Every character the renderer
/// emits is one column wide; a label is a URL, which is ASCII.
pub fn row_width(row: &[Segment]) -> usize {
    row.iter()
        .map(|segment| segment.text().chars().count())
        .sum()
}

/// One code with its quiet zone, as half-block rows of equal width. `None`
/// when `text` is too long for any QR code.
///
/// Error correction is the LOW level: a code on a screen is not going to be
/// scuffed or folded, and the lower level keeps a typical tailnet URL a size
/// smaller, which is what lets two codes sit side by side on 80 columns.
///
/// A code is an odd number of modules wide, so the last row's lower half falls
/// outside it; it is drawn as quiet zone, which only makes the bottom margin
/// half a module taller.
pub fn code_rows(text: &str, polarity: Polarity) -> Option<Vec<String>> {
    let code =
        qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::L).ok()?;
    let width = code.width();
    let colors = code.to_colors();
    let size = width + 2 * QUIET_ZONE;
    let dark = |x: usize, y: usize| -> bool {
        let inside = (QUIET_ZONE..width + QUIET_ZONE).contains(&x)
            && (QUIET_ZONE..width + QUIET_ZONE).contains(&y);
        inside && colors[(y - QUIET_ZONE) * width + (x - QUIET_ZONE)] == qrcode::Color::Dark
    };
    let filled = |x: usize, y: usize| match polarity {
        Polarity::DarkModulesFilled => dark(x, y),
        Polarity::LightModulesFilled => !dark(x, y),
    };
    let rows = (0..size)
        .step_by(2)
        .map(|y| {
            (0..size)
                .map(|x| match (filled(x, y), filled(x, y + 1)) {
                    (true, true) => '█',
                    (true, false) => '▀',
                    (false, true) => '▄',
                    (false, false) => ' ',
                })
                .collect()
        })
        .collect();
    Some(rows)
}

/// How several codes are placed in the space available.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrangement {
    SideBySide,
    Stacked,
}

/// Side by side when every column (a code or its label, whichever is wider)
/// fits in `available` columns with [`COLUMN_GAP`] between them, stacked
/// otherwise.
pub fn arrangement(column_widths: &[usize], available: usize) -> Arrangement {
    let gaps = COLUMN_GAP * column_widths.len().saturating_sub(1);
    let needed: usize = column_widths.iter().sum::<usize>() + gaps;
    if needed <= available {
        Arrangement::SideBySide
    } else {
        Arrangement::Stacked
    }
}

/// Lay out one code per URL, each labelled with its URL under it, side by side
/// when they fit in `available` columns and stacked when they do not. A URL too
/// long for a QR code is left out.
pub fn layout(urls: &[&str], available: usize, polarity: Polarity) -> Vec<Row> {
    let panels: Vec<Panel> = urls
        .iter()
        .filter_map(|url| {
            let code = code_rows(url, polarity)?;
            Some(Panel {
                code_width: code.first().map_or(0, |row| row.chars().count()),
                code,
                label: (*url).to_string(),
            })
        })
        .collect();
    if panels.is_empty() {
        return Vec::new();
    }
    let widths: Vec<usize> = panels.iter().map(Panel::column_width).collect();
    match arrangement(&widths, available) {
        Arrangement::SideBySide => side_by_side(&panels),
        Arrangement::Stacked => stacked(&panels),
    }
}

/// One code and the URL under it.
struct Panel {
    code: Vec<String>,
    code_width: usize,
    label: String,
}

impl Panel {
    /// The code or its label, whichever is wider.
    fn column_width(&self) -> usize {
        self.code_width.max(self.label.chars().count())
    }
}

fn spaces(count: usize) -> Segment {
    Segment::Text(" ".repeat(count))
}

/// Every panel in one band, each code centred over its label in its own column.
fn side_by_side(panels: &[Panel]) -> Vec<Row> {
    let height = panels.iter().map(|p| p.code.len()).max().unwrap_or(0);
    let mut rows: Vec<Row> = Vec::with_capacity(height + 1);
    for line in 0..height {
        let mut row = Vec::new();
        for (index, panel) in panels.iter().enumerate() {
            if index > 0 {
                row.push(spaces(COLUMN_GAP));
            }
            let column = panel.column_width();
            let left = (column - panel.code_width) / 2;
            if left > 0 {
                row.push(spaces(left));
            }
            match panel.code.get(line) {
                Some(code) => row.push(Segment::Code(code.clone())),
                None => row.push(spaces(panel.code_width)),
            }
            let right = column - panel.code_width - left;
            if right > 0 && index + 1 < panels.len() {
                row.push(spaces(right));
            }
        }
        rows.push(row);
    }
    let mut labels = Vec::new();
    for (index, panel) in panels.iter().enumerate() {
        if index > 0 {
            labels.push(spaces(COLUMN_GAP));
        }
        let column = panel.column_width();
        let label_width = panel.label.chars().count();
        let left = (column - label_width) / 2;
        let right = column - label_width - left;
        if left > 0 {
            labels.push(spaces(left));
        }
        labels.push(Segment::Text(panel.label.clone()));
        if right > 0 && index + 1 < panels.len() {
            labels.push(spaces(right));
        }
    }
    rows.push(labels);
    rows
}

/// One panel after another, a blank row between them.
fn stacked(panels: &[Panel]) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for (index, panel) in panels.iter().enumerate() {
        if index > 0 {
            rows.push(Vec::new());
        }
        rows.extend(
            panel
                .code
                .iter()
                .map(|code| vec![Segment::Code(code.clone())]),
        );
        rows.push(vec![Segment::Text(panel.label.clone())]);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read the modules back out of the drawn rows: `true` is a dark module.
    fn modules(rows: &[String], polarity: Polarity) -> Vec<Vec<bool>> {
        let mut grid = Vec::new();
        for row in rows {
            let mut top = Vec::new();
            let mut bottom = Vec::new();
            for ch in row.chars() {
                let (t, b) = match ch {
                    '█' => (true, true),
                    '▀' => (true, false),
                    '▄' => (false, true),
                    ' ' => (false, false),
                    other => panic!("unexpected character {other:?} in a code row"),
                };
                let dark = |filled: bool| match polarity {
                    Polarity::DarkModulesFilled => filled,
                    Polarity::LightModulesFilled => !filled,
                };
                top.push(dark(t));
                bottom.push(dark(b));
            }
            grid.push(top);
            grid.push(bottom);
        }
        grid
    }

    /// Decode the drawn code the way a phone would: as pixels, with no help.
    fn decode(rows: &[String], polarity: Polarity) -> String {
        let grid = modules(rows, polarity);
        const SCALE: usize = 6;
        const MARGIN: usize = 4;
        let size = grid.len();
        let px = (size + 2 * MARGIN) * SCALE;
        let mut image = rqrr::PreparedImage::prepare_from_greyscale(px, px, |x, y| {
            let (mx, my) = (x / SCALE, y / SCALE);
            if mx < MARGIN || my < MARGIN || mx >= size + MARGIN || my >= size + MARGIN {
                return 255;
            }
            let row = &grid[my - MARGIN];
            match row.get(mx - MARGIN) {
                Some(true) => 0,
                _ => 255,
            }
        });
        let grids = image.detect_grids();
        assert_eq!(grids.len(), 1, "exactly one code must be found");
        grids[0].decode().expect("the code decodes").1
    }

    const IP_URL: &str = "http://100.101.102.103:3890";
    const NAME_URL: &str = "https://demo-box.example-tailnet.ts.net";

    #[test]
    fn a_drawn_code_decodes_back_to_its_url_in_either_polarity() {
        for polarity in [Polarity::DarkModulesFilled, Polarity::LightModulesFilled] {
            for url in [
                IP_URL,
                NAME_URL,
                "https://demo-box.example-tailnet.ts.net:8443",
            ] {
                let rows = code_rows(url, polarity).expect("a URL fits");
                assert_eq!(decode(&rows, polarity), url, "{polarity:?}");
            }
        }
    }

    #[test]
    fn a_code_is_square_with_its_quiet_zone_and_two_modules_per_row() {
        let rows = code_rows(NAME_URL, Polarity::DarkModulesFilled).unwrap();
        let width = rows[0].chars().count();
        assert!(
            rows.iter().all(|r| r.chars().count() == width),
            "every row is as wide as the first"
        );
        // A QR code is 17 + 4v modules for version v, plus the quiet zone on
        // both sides; a row carries two module rows, rounding up.
        let modules = width - 2 * QUIET_ZONE;
        assert_eq!((modules - 17) % 4, 0, "{modules} is a QR code size");
        assert_eq!(rows.len(), width.div_ceil(2));
    }

    #[test]
    fn the_quiet_zone_is_light_on_every_side() {
        let rows = code_rows(IP_URL, Polarity::DarkModulesFilled).unwrap();
        let grid = modules(&rows, Polarity::DarkModulesFilled);
        let size = rows[0].chars().count();
        let dark_at = |x: usize, y: usize| grid[y][x];
        for q in 0..QUIET_ZONE {
            for i in 0..size {
                assert!(!dark_at(i, q), "top quiet row {q}");
                assert!(!dark_at(q, i), "left quiet column {q}");
                assert!(!dark_at(size - 1 - q, i), "right quiet column {q}");
                assert!(!dark_at(i, size - 1 - q), "bottom quiet row {q}");
            }
        }
    }

    #[test]
    fn the_same_url_always_draws_the_same_code() {
        let a = code_rows(IP_URL, Polarity::DarkModulesFilled).unwrap();
        let b = code_rows(IP_URL, Polarity::DarkModulesFilled).unwrap();
        assert_eq!(a, b);
        let drawn: Vec<String> = a.iter().map(|row| row.replace(' ', "·")).collect();
        assert_eq!(
            drawn, SNAPSHOT_IP_URL,
            "the drawing of a fixed URL is stable"
        );
    }

    #[test]
    fn text_too_long_for_any_code_draws_nothing() {
        let huge = "x".repeat(8000);
        assert_eq!(code_rows(&huge, Polarity::DarkModulesFilled), None);
        assert!(layout(&[&huge], 200, Polarity::DarkModulesFilled).is_empty());
    }

    #[test]
    fn two_codes_go_side_by_side_exactly_when_both_columns_and_the_gap_fit() {
        assert_eq!(
            arrangement(&[37, 41], 37 + COLUMN_GAP + 41),
            Arrangement::SideBySide
        );
        assert_eq!(
            arrangement(&[37, 41], 37 + COLUMN_GAP + 41 - 1),
            Arrangement::Stacked
        );
        assert_eq!(
            arrangement(&[37], 37),
            Arrangement::SideBySide,
            "one code always fits its own column"
        );
        assert_eq!(arrangement(&[], 10), Arrangement::SideBySide);
    }

    #[test]
    fn side_by_side_codes_carry_their_urls_under_them() {
        let rows = layout(&[IP_URL, NAME_URL], 200, Polarity::DarkModulesFilled);
        let last = row_text(rows.last().unwrap());
        let ip_at = last.find(IP_URL).expect("the IP label");
        let name_at = last.find(NAME_URL).expect("the name label");
        assert!(
            ip_at < name_at,
            "left to right in the order given: {last:?}"
        );
        let code_row = &rows[0];
        let codes = code_row
            .iter()
            .filter(|s| matches!(s, Segment::Code(_)))
            .count();
        assert_eq!(codes, 2, "two codes on one row");
        let widest = rows.iter().map(|r| row_width(r)).max().unwrap();
        assert!(widest <= 200);
    }

    #[test]
    fn stacked_codes_each_carry_their_url_under_them_and_fit_the_width() {
        let ip_rows = code_rows(IP_URL, Polarity::DarkModulesFilled).unwrap();
        let ip_width = ip_rows[0].chars().count();
        let name_width = code_rows(NAME_URL, Polarity::DarkModulesFilled).unwrap()[0]
            .chars()
            .count()
            .max(NAME_URL.len());
        let available = ip_width.max(IP_URL.len()) + COLUMN_GAP + name_width - 1;
        let rows = layout(&[IP_URL, NAME_URL], available, Polarity::DarkModulesFilled);
        let texts: Vec<String> = rows.iter().map(|r| row_text(r)).collect();
        let ip_label = texts.iter().position(|t| t.contains(IP_URL)).unwrap();
        let name_label = texts.iter().position(|t| t.contains(NAME_URL)).unwrap();
        assert_eq!(
            ip_label,
            ip_rows.len(),
            "the first label sits right under its code"
        );
        assert!(
            name_label > ip_label + 1,
            "the second code comes after the first label"
        );
        for row in &rows {
            assert!(
                rows.iter()
                    .all(|r| r.iter().filter(|s| matches!(s, Segment::Code(_))).count() <= 1),
                "one code per row when stacked"
            );
            assert!(row_width(row) <= available, "{:?}", row_text(row));
        }
    }

    #[test]
    fn every_code_in_a_layout_still_decodes() {
        let rows = layout(&[IP_URL, NAME_URL], 200, Polarity::LightModulesFilled);
        // Cut each code back out of the side-by-side rows by the column it
        // starts at: the codes can be different sizes, so a shorter one simply
        // has no segment on the rows below it.
        let mut by_column: std::collections::BTreeMap<usize, Vec<String>> =
            std::collections::BTreeMap::new();
        for row in &rows {
            let mut column = 0;
            for segment in row {
                if let Segment::Code(text) = segment {
                    by_column.entry(column).or_default().push(text.clone());
                }
                column += segment.text().chars().count();
            }
        }
        let decoded: Vec<String> = by_column
            .values()
            .map(|code| decode(code, Polarity::LightModulesFilled))
            .collect();
        assert_eq!(decoded, vec![IP_URL.to_string(), NAME_URL.to_string()]);
    }

    #[test]
    fn no_urls_lay_out_to_nothing() {
        assert!(layout(&[], 80, Polarity::DarkModulesFilled).is_empty());
    }

    /// The code for [`IP_URL`], spaces written as `·` so no editor can trim
    /// the quiet zone off the end of a line.
    const SNAPSHOT_IP_URL: &[&str] = &[
        "·····························",
        "··█▀▀▀▀▀█·▄▀·▄█▄·██·█▀▀▀▀▀█··",
        "··█·███·█·▀███▀··▄█·█·███·█··",
        "··█·▀▀▀·█·█▄·▄··▀█▄·█·▀▀▀·█··",
        "··▀▀▀▀▀▀▀·█▄▀·█·▀▄█·▀▀▀▀▀▀▀··",
        "··█▀·▀··▀█··█▄▀█▄···██▀·▀▀▄··",
        "···▄▀▄█·▀█▀▀▀·▀█▀·▄·▀██··▀▀··",
        "··▀▀▀▀·█▀▀█▄▄▄▀▀█▀█▄█▀·█▄▀█··",
        "··▀▄▄▄▀▀▀█▀▄▄█▄▄·▄·▄▄█·▄▀▄▀··",
        "··▀▀▀·▀·▀▀█▄▀·▀▄▀·█▀▀▀███·▄··",
        "··█▀▀▀▀▀█·▀▀·█·▀███·▀·█▄▄▀█··",
        "··█·███·█·▄███·▀▄▄▀█▀▀█·▄·▀··",
        "··█·▀▀▀·█·▄·█·▀▄█·▄▀·▀▀█▀·▀··",
        "··▀▀▀▀▀▀▀·▀·····▀·▀▀▀▀···▀▀··",
        "·····························",
    ];
}
