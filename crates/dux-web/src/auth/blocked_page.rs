//! What a fresh page load from a blocked address gets instead of the JSON the
//! app reads (decided: it has no app to read it yet).
//!
//! One self-contained page, `blocked_page.html`: inline styles in the web UI's
//! own dark theme colours, no script, and nothing to fetch, because a blocked
//! address is refused every asset too. The duck therefore rides in the page as
//! a data URI, encoded at compile time from the same `favicon.png` the app's
//! other sign-in pages show. The app's own blocked page says the same thing in
//! the same words, which `LoginPage.test.tsx` checks against this file.

use std::sync::LazyLock;

/// The logo every sign-in page shows, read from the web UI's own source.
const LOGO_PNG: &[u8] = include_bytes!("../../web/public/favicon.png");

const LOGO_BASE64_LEN: usize = LOGO_PNG.len().div_ceil(3) * 4;

static LOGO_BASE64: [u8; LOGO_BASE64_LEN] = base64_encode(LOGO_PNG);

const TEMPLATE: &str = include_str!("blocked_page.html");

const LOGO_SLOT: &str = "{{LOGO_DATA_URI}}";

/// Standard base64 with padding, in a `const fn` so the page's logo is built
/// at compile time from the bytes above.
const fn base64_encode<const N: usize>(input: &[u8]) -> [u8; N] {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = [b'='; N];
    let mut i = 0;
    let mut o = 0;
    while i < input.len() {
        let b0 = input[i] as u32;
        let b1 = if i + 1 < input.len() {
            input[i + 1] as u32
        } else {
            0
        };
        let b2 = if i + 2 < input.len() {
            input[i + 2] as u32
        } else {
            0
        };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out[o] = ALPHABET[((triple >> 18) & 63) as usize];
        out[o + 1] = ALPHABET[((triple >> 12) & 63) as usize];
        if i + 1 < input.len() {
            out[o + 2] = ALPHABET[((triple >> 6) & 63) as usize];
        }
        if i + 2 < input.len() {
            out[o + 3] = ALPHABET[(triple & 63) as usize];
        }
        i += 3;
        o += 4;
    }
    out
}

static LOGO_BASE64_STR: &str = match std::str::from_utf8(&LOGO_BASE64) {
    Ok(s) => s,
    Err(_) => panic!("base64 is ASCII"),
};

/// The page, with the logo in its slot.
pub(crate) static BLOCKED_PAGE: LazyLock<String> = LazyLock::new(|| {
    TEMPLATE.replace(
        LOGO_SLOT,
        &format!("data:image/png;base64,{LOGO_BASE64_STR}"),
    )
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_like_the_standard_alphabet_with_padding() {
        assert_eq!(&base64_encode::<0>(b""), b"");
        assert_eq!(&base64_encode::<4>(b"f"), b"Zg==");
        assert_eq!(&base64_encode::<4>(b"fo"), b"Zm8=");
        assert_eq!(&base64_encode::<4>(b"foo"), b"Zm9v");
        assert_eq!(&base64_encode::<8>(b"foob"), b"Zm9vYg==");
        assert_eq!(&base64_encode::<8>(b"fooba"), b"Zm9vYmE=");
        assert_eq!(&base64_encode::<8>(b"foobar"), b"Zm9vYmFy");
        assert_eq!(&base64_encode::<4>(&[0xff, 0xfe, 0xfd]), b"//79");
    }

    #[test]
    fn the_page_carries_the_png_whole_and_no_slot_is_left_unfilled() {
        assert!(LOGO_PNG.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(LOGO_BASE64_STR.starts_with("iVBORw0KGgo"));
        assert!(BLOCKED_PAGE.contains(&format!("src=\"data:image/png;base64,{LOGO_BASE64_STR}\"")));
        assert!(!BLOCKED_PAGE.contains("{{"));
        assert_eq!(TEMPLATE.matches(LOGO_SLOT).count(), 1);
    }
}
