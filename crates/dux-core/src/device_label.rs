//! Turning a recorded driver identity into a SHORT label a surface can render.
//!
//! The PTY-ownership registry records whatever a claiming connection presented
//! at its upgrade: for a browser that is its raw `User-Agent`, which is
//! routinely 120 to 200 characters long, and for the terminal UI it is the fixed
//! [`TUI_DEVICE_LABEL`]. Both end up as COPY on a watcher's screen, and a real
//! `User-Agent` does not fit where a surface has to put it: the title bar of a
//! card inside a pane, or a card in a browser column, both of which are a name's
//! worth of room and not a paragraph's.
//!
//! This is the terminal UI's twin of the web's `deviceLabel.ts`, and the two are
//! deliberately kept in step: the same UA shapes must produce the same label on
//! both surfaces, or one device is called two different things depending on which
//! screen is looking at it. The tests below mirror the web's fixtures literally
//! so a change to one parser that the other did not follow fails here rather
//! than silently drifting.
//!
//! ## Where the two deliberately differ
//!
//! `deviceLabel.ts` returns `null` for anything it cannot parse, and its caller
//! renders a generic fallback. This returns a TRUNCATED, control-stripped prefix
//! of the raw string instead. The reason is the surface: the web renders into
//! HTML, where echoing an attacker-supplied string is a class of bug all by
//! itself, while this renders into a fixed-width themed span whose contents are
//! stripped of control bytes and bounded to [`SHORT_LABEL_MAX_CHARS`] display
//! characters first. A truncated prefix is more use than "another device" when
//! someone is trying to work out which of their own machines is holding a
//! terminal, and it cannot corrupt the frame.

use crate::background_serve::TUI_DEVICE_LABEL;

/// How long an UNRECOGNIZED label may be before it is cut, ellipsis included.
///
/// Sized for the place it is rendered: one line inside the center pane, sharing
/// that line with the sentence that says how to take the terminal back. Names
/// this parser DOES recognize ("Chrome on Android") already sit well inside it.
pub const SHORT_LABEL_MAX_CHARS: usize = 24;

/// The operating system a `User-Agent` names, or `None`.
///
/// ORDER MATTERS, and it is the same order `deviceLabel.ts` uses: an Android UA
/// also contains "Linux" and an iOS UA also contains "like Mac OS X", so the
/// more specific token has to be tested first.
fn detect_os(ua: &str) -> Option<&'static str> {
    if ua.contains("Android") {
        return Some("Android");
    }
    if ua.contains("iPhone") || ua.contains("iPad") || ua.contains("iPod") {
        return Some("iOS");
    }
    if ua.contains("Windows") {
        return Some("Windows");
    }
    if ua.contains("Macintosh") || ua.contains("Mac OS X") {
        return Some("macOS");
    }
    if ua.contains("Linux") {
        return Some("Linux");
    }
    None
}

/// The browser a `User-Agent` names, or `None`.
///
/// ORDER MATTERS here too, for the same reason it does on the web side: Edge and
/// Chrome both carry a "Chrome/" token and Chrome and Safari both carry a
/// "Safari/" token, so each check has to exclude the engines layered above it.
/// Edge's token is platform specific ("Edg/" on the desktop, "EdgA/" on Android,
/// "EdgiOS/" on iOS) and none of the mobile spellings contain the bare desktop
/// one, so they are matched explicitly or they fall through to Chrome.
/// Chromium-based browsers (Opera, Brave, Vivaldi) are folded into Chrome, which
/// is what the web side does as well.
fn detect_browser(ua: &str) -> Option<&'static str> {
    if ua.contains("Edg/") || ua.contains("EdgA/") || ua.contains("EdgiOS/") {
        return Some("Edge");
    }
    if ua.contains("Firefox/") {
        return Some("Firefox");
    }
    if ua.contains("Chrome/") || ua.contains("CriOS/") {
        return Some("Chrome");
    }
    if ua.contains("Safari/") {
        return Some("Safari");
    }
    None
}

/// Cut `text` to at most `max` DISPLAY characters, marking the cut.
///
/// Char based, never byte based: a `User-Agent` is attacker-supplied and can
/// carry multi-byte UTF-8, and byte slicing inside a character panics.
///
/// Public because the surface that renders a label has a second, narrower budget
/// the parser cannot know: the width actually left on the line after the rest of
/// the sentence. Cutting twice with one function keeps the ellipsis consistent.
pub fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    // One character of the budget belongs to the ellipsis that says it was cut.
    let keep = max.saturating_sub(1);
    let mut out: String = text.chars().take(keep).collect();
    out.push('\u{2026}');
    out
}

/// The short label for a recorded driver identity, or `None` when there is
/// nothing to name at all.
///
///   - Nothing, or only whitespace and control bytes, gives `None`; the caller
///     says "another device", which is the honest answer for a connection that
///     presented no `User-Agent`.
///   - The terminal UI's own fixed label passes through verbatim. It parses as
///     no OS at all, so without this the one non-browser driver dux has would be
///     truncated into nonsense.
///   - A known OS with a known browser gives "Chrome on macOS".
///   - A known OS alone gives the OS.
///   - Anything else gives a control-stripped prefix, cut to
///     [`SHORT_LABEL_MAX_CHARS`].
pub fn short_device_label(raw: &str) -> Option<String> {
    // Control bytes first, before anything measures or matches: this value is
    // rendered into a frame, and an escape sequence smuggled through a
    // `User-Agent` would move the cursor rather than print. Stripping is the
    // right answer rather than refusing, because the printable remainder is
    // still the honest name of the device.
    let cleaned: String = raw.chars().filter(|c| !c.is_control()).collect();
    let ua = cleaned.trim();
    if ua.is_empty() {
        return None;
    }
    if ua == TUI_DEVICE_LABEL {
        return Some(ua.to_string());
    }
    match detect_os(ua) {
        Some(os) => Some(match detect_browser(ua) {
            Some(browser) => format!("{browser} on {os}"),
            None => os.to_string(),
        }),
        None => Some(truncate_chars(ua, SHORT_LABEL_MAX_CHARS)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The web's own fixtures, mirrored LITERALLY from
    /// `crates/dux-web/web/src/lib/deviceLabel.test.ts`. Copied rather than
    /// referenced because the two parsers are in two languages: the point of the
    /// duplication is that a change to either parser that the other did not
    /// follow shows up as a failing assertion here.
    mod ua {
        pub const CHROME_MAC: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
             AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
        pub const CHROME_LINUX: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
        pub const CHROME_WINDOWS: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
             AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
        pub const SAFARI_MAC: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
             AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.1 Safari/605.1.15";
        pub const FIREFOX_LINUX: &str =
            "Mozilla/5.0 (X11; Linux x86_64; rv:121.0) Gecko/20100101 Firefox/121.0";
        pub const EDGE_WINDOWS: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
             AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36 \
             Edg/120.0.0.0";
        pub const CHROME_ANDROID: &str = "Mozilla/5.0 (Linux; Android 13; Pixel 7) \
             AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36";
        pub const SAFARI_IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_1 like Mac OS X) \
             AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.1 Mobile/15E148 Safari/604.1";
        pub const EDGE_ANDROID: &str = "Mozilla/5.0 (Linux; Android 13; Pixel 7) \
             AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36 \
             EdgA/120.0.0.0";
        pub const EDGE_IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_1 like Mac OS X) \
             AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.1 EdgiOS/120.0.0.0 \
             Mobile/15E148 Safari/605.1.15";
        pub const UNKNOWN_BROWSER_WINDOWS: &str = "SomeCrawler/2.0 (Windows NT 10.0; Win64; x64)";
    }

    /// The shared vector: every pair the web's own test asserts must produce the
    /// SAME label here. This is the anti-drift test.
    #[test]
    fn the_web_fixtures_produce_the_same_labels_here() {
        let cases = [
            (ua::CHROME_MAC, "Chrome on macOS"),
            (ua::CHROME_LINUX, "Chrome on Linux"),
            (ua::CHROME_WINDOWS, "Chrome on Windows"),
            (ua::SAFARI_MAC, "Safari on macOS"),
            (ua::FIREFOX_LINUX, "Firefox on Linux"),
            (ua::EDGE_WINDOWS, "Edge on Windows"),
            (ua::CHROME_ANDROID, "Chrome on Android"),
            (ua::SAFARI_IPHONE, "Safari on iOS"),
            // Mobile Edge: the bare desktop "Edg/" would miss both of these and
            // they would come back as Chrome, which is the same bug the web side
            // fixed by broadening its pattern.
            (ua::EDGE_ANDROID, "Edge on Android"),
            (ua::EDGE_IPHONE, "Edge on iOS"),
            // A known OS whose browser token this parser does not know.
            (ua::UNKNOWN_BROWSER_WINDOWS, "Windows"),
        ];
        for (raw, expected) in cases {
            assert_eq!(
                short_device_label(raw).as_deref(),
                Some(expected),
                "the web's fixture for {expected} must produce the same label here"
            );
        }
    }

    /// The terminal UI is the one participant that is not a browser, and its
    /// fixed label is copy already: it passes through untouched, exactly as the
    /// web's exact-match rule passes it through.
    #[test]
    fn the_terminal_uis_own_label_passes_through_verbatim() {
        assert_eq!(
            short_device_label(TUI_DEVICE_LABEL).as_deref(),
            Some(TUI_DEVICE_LABEL)
        );
    }

    /// Nothing to name is `None`, so the caller can say "another device" rather
    /// than render an empty span where a device name belongs.
    #[test]
    fn an_empty_or_blank_identity_names_nothing() {
        assert_eq!(short_device_label(""), None);
        assert_eq!(short_device_label("    "), None);
        // Control bytes alone leave nothing printable behind.
        assert_eq!(short_device_label("\u{1b}\u{7}\r\n"), None);
    }

    /// THE BUG THIS EXISTS FOR. A real `User-Agent` is far longer than the line
    /// it is rendered on, so every recognized shape must come back SHORT.
    #[test]
    fn every_recognized_shape_fits_the_line_it_is_rendered_on() {
        for raw in [
            ua::CHROME_MAC,
            ua::EDGE_IPHONE,
            ua::SAFARI_IPHONE,
            ua::CHROME_ANDROID,
        ] {
            let label = short_device_label(raw).expect("a real UA names a device");
            assert!(
                raw.chars().count() > SHORT_LABEL_MAX_CHARS,
                "the fixture has to be longer than the budget or it proves nothing"
            );
            assert!(
                label.chars().count() <= SHORT_LABEL_MAX_CHARS,
                "{label:?} is {} characters, over the {SHORT_LABEL_MAX_CHARS} the pane has",
                label.chars().count()
            );
        }
    }

    /// An unrecognized identity is cut to the budget rather than refused, and the
    /// cut is marked so nobody reads a prefix as the whole name.
    #[test]
    fn an_unrecognized_identity_is_cut_to_the_budget_and_marked() {
        let label = short_device_label(&"z".repeat(200)).expect("something was said");
        assert_eq!(label.chars().count(), SHORT_LABEL_MAX_CHARS);
        assert!(
            label.ends_with('\u{2026}'),
            "the cut must be visible: {label}"
        );

        // Short enough already: kept whole, with no ellipsis invented for it.
        assert_eq!(
            short_device_label("my-laptop").as_deref(),
            Some("my-laptop")
        );
    }

    /// The cut is CHARACTER based. A byte-based cut of multi-byte text panics
    /// inside a character, and this input is attacker-supplied.
    #[test]
    fn the_cut_never_lands_inside_a_multibyte_character() {
        let label = short_device_label(&"\u{1f600}".repeat(40)).expect("something was said");
        assert_eq!(label.chars().count(), SHORT_LABEL_MAX_CHARS);
        assert!(label.starts_with('\u{1f600}'));
    }

    /// Control bytes are stripped BEFORE anything renders the label: this value
    /// goes into a frame, and a smuggled escape sequence would move the cursor
    /// instead of printing.
    #[test]
    fn control_bytes_are_stripped_rather_than_rendered() {
        let label = short_device_label("lab\u{1b}[31mtop\u{7}").expect("something was said");
        assert!(
            !label.chars().any(char::is_control),
            "no control byte may survive into the frame: {label:?}"
        );
        assert_eq!(label, "lab[31mtop");
    }
}
