//! Text encodings for the web editor: turning a file's bytes into the text the
//! editor shows, and the edited text back into bytes in the same encoding.
//!
//! There is no guessing beyond a byte-order mark. A file with a mark is read in
//! the encoding the mark names; a file without one that is valid UTF-8 is UTF-8;
//! anything else is read as windows-1252 (the WHATWG name ISO-8859-1 resolves
//! to), which maps every byte to a character and so loses nothing, and the
//! answer says that it was a fallback rather than a finding. The person reading
//! the file is the one who can tell it is really `Shift_JIS`, so a client may ask
//! for a specific encoding instead, and then exactly that one is used: a file
//! that does not decode cleanly in it is refused, because replacement
//! characters on screen would be written back over the real bytes by a save.
//!
//! A decode is only half the promise. Some encodings do not write back what
//! they read (`Shift_JIS` spells some characters two ways, ISO-2022-JP moves its
//! escapes), so every text answer is checked by encoding it again, and a file
//! that would not come back byte for byte is marked unwritable in that encoding:
//! the editor shows it read-only, and another encoding is the way out.
//!
//! Saving runs the other way and refuses just as plainly: a character the
//! target encoding cannot hold stops the save before anything is written, and
//! the refusal names the character and where it is.

use encoding_rs::{DecoderResult, EncoderResult, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252};

/// Re-exported so the web routes name the type without a dependency of their own.
pub use encoding_rs::Encoding;

/// What a file's bytes are as editor text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decoded {
    Text(DecodedText),
    /// Not text in any sense the editor can round-trip: a NUL, a known binary
    /// signature, or a byte-order mark whose promise the bytes do not keep and
    /// that then fails the ordinary rules too.
    Binary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedText {
    pub text: String,
    pub encoding: &'static Encoding,
    /// The file began with a byte-order mark for `encoding`, which is not part
    /// of `text` and has to be written back by a save.
    pub bom: bool,
    /// The encoding is the last resort for bytes that are not UTF-8, not
    /// something the file said about itself.
    pub fallback: bool,
    /// Why a save in this encoding could not write the file back as it is, so
    /// the editor must show it read-only. `None` for the ordinary case.
    pub unwritable: Option<Unwritable>,
}

/// Why a file that reads as text in an encoding still cannot be saved in it
/// without changing bytes nobody edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unwritable {
    /// Encoding the decoded text gives different bytes from the file's: some
    /// sequences have two spellings in this encoding, or none to go back to.
    RoundTrip,
    /// The text starts with a second byte-order mark, which the editor drops
    /// from what it holds, so a save would write one mark fewer.
    DoubledMark,
    /// The text starts with the character U+FEFF and no mark was set aside
    /// before it (gb18030 spells it 84 31 95 33, say); the editor drops it the
    /// same way, so a save would lose it.
    LeadingMark,
}

impl Unwritable {
    /// The wire name of the reason.
    pub fn as_str(self) -> &'static str {
        match self {
            Unwritable::RoundTrip => "round_trip",
            Unwritable::DoubledMark => "doubled_mark",
            Unwritable::LeadingMark => "leading_mark",
        }
    }
}

/// A save refused because the text starts with U+FEFF in UTF-8 or UTF-16, where
/// that character is written as the very bytes of a byte-order mark: a read
/// would take it as the file's mark, so the text saved and the text read back
/// would differ. The mark is chosen on its own (the encoding's "with BOM"
/// row), never typed. Nothing is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeadingMarkInText {
    pub encoding: &'static Encoding,
    /// The file itself starts with that character, read in the encoding the
    /// save says it was read in, rather than the person typing it. Such a file
    /// opens read-only, so there is nothing to delete and the only way to
    /// change it is outside dux.
    pub from_file: bool,
}

impl std::fmt::Display for LeadingMarkInText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.from_file {
            return write!(
                f,
                "refusing to convert: this file starts with the character U+FEFF, which {} can \
                 only write as a byte-order mark, so the converted file would read back \
                 without it; nothing was written. dux can't convert this file; edit it outside \
                 dux",
                self.encoding.name()
            );
        }
        write!(
            f,
            "refusing to save: the text starts with the character U+FEFF, which {} writes as \
             a byte-order mark, so the file would read back without it; nothing was written. \
             Delete that character and save again; a byte-order mark is chosen with Save with \
             encoding, never typed",
            self.encoding.name()
        )
    }
}

impl std::error::Error for LeadingMarkInText {}

/// A save refused because the file on disk reads in the save's encoding but
/// that encoding cannot write it back unchanged: saving would change bytes the
/// editor never showed as changed. Nothing is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WouldRewriteUntouched {
    pub encoding: &'static Encoding,
    pub reason: Unwritable,
}

impl std::fmt::Display for WouldRewriteUntouched {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let why = match self.reason {
            Unwritable::RoundTrip => {
                "it would write some of the file's own characters back as \
                                      different bytes"
            }
            Unwritable::DoubledMark => {
                "the file starts with two byte-order marks and a save \
                                        would drop one"
            }
            Unwritable::LeadingMark => {
                "the file starts with a byte-order mark character a save \
                                        would drop"
            }
        };
        write!(
            f,
            "refusing to save in {}: {why}; nothing was written. Reopen the file in another \
             encoding, or save it in another one to convert it",
            self.encoding.name()
        )
    }
}

impl std::error::Error for WouldRewriteUntouched {}

/// What a read of a just-saved file would say about it, judged without reading
/// it back: whether its encoding writes it back unchanged, and its line endings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavedVerdict {
    pub unwritable: Option<Unwritable>,
    pub line_endings: LineEndings,
}

/// The verdict on `text` saved in `encoding` (with a mark when `bom`): what a
/// read of the bytes written would say, so the editor never assumes how a save
/// left the file: a conversion is not assumed writable in its new encoding.
pub fn saved_verdict(text: &str, encoding: &'static Encoding, bom: bool) -> SavedVerdict {
    let read =
        encode(text, encoding, bom)
            .ok()
            .and_then(|bytes| match decode(&bytes, Some(encoding)) {
                Ok(Decoded::Text(t)) => Some(t),
                _ => None,
            });
    SavedVerdict {
        unwritable: read
            .as_ref()
            .map_or(Some(Unwritable::RoundTrip), |t| t.unwritable),
        line_endings: line_endings(text),
    }
}

/// Whether a guarded save in `encoding` over `on_disk` would change bytes the
/// file holds that nobody edited: true when those bytes read in `encoding` and
/// that encoding cannot write them back. Bytes that do not read in `encoding`
/// at all are a conversion from another one, which rewrites the whole file on
/// purpose, and are not this question.
pub fn would_rewrite_untouched(on_disk: &[u8], encoding: &'static Encoding) -> Option<Unwritable> {
    match decode(on_disk, Some(encoding)) {
        Ok(Decoded::Text(t)) => t.unwritable,
        _ => None,
    }
}

/// A requested encoding the file does not decode in. Reported rather than
/// papered over with replacement characters, which a save would write back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeRefused {
    pub encoding: &'static Encoding,
    /// Byte offset of the first sequence that is not valid in `encoding`.
    pub offset: usize,
}

impl std::fmt::Display for DecodeRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the file is not valid {}: the bytes at offset {} are not a character in \
             that encoding",
            self.encoding.name(),
            self.offset
        )
    }
}

/// A line ending style, as the editor writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eol {
    Lf,
    Crlf,
}

impl Eol {
    pub fn label(self) -> &'static str {
        match self {
            Eol::Lf => "LF",
            Eol::Crlf => "CRLF",
        }
    }
}

/// What a text's line endings are, and what the editor will write them as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineEndings {
    /// More than one style, or a lone CR: the editor holds one style per
    /// file, so a save rewrites the ones that differ.
    pub mixed: bool,
    /// The one style the editor keeps and a save writes.
    pub saves_as: Eol,
}

/// Classify `text`'s line endings the way the browser's editor (Monaco) does
/// when it loads a file: a lone CR and a CRLF both count as carriage returns,
/// and CRLF wins only when more than half of all breaks carry one. A text with
/// no breaks at all is written as LF, the editor's default.
pub fn line_endings(text: &str) -> LineEndings {
    let bytes = text.as_bytes();
    let (mut lf, mut crlf, mut cr) = (0usize, 0usize, 0usize);
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' if bytes.get(i + 1) == Some(&b'\n') => {
                crlf += 1;
                i += 1;
            }
            b'\r' => cr += 1,
            b'\n' => lf += 1,
            _ => {}
        }
        i += 1;
    }
    let total = lf + crlf + cr;
    let saves_as = if total > 0 && (cr + crlf) * 2 > total {
        Eol::Crlf
    } else {
        Eol::Lf
    };
    LineEndings {
        mixed: cr > 0 || (lf > 0 && crlf > 0),
        saves_as,
    }
}

/// Whether `bytes` are valid UTF-8 holding something beyond ASCII: the bytes a
/// file rewritten as UTF-8 would have, which no legacy encoding spells by
/// accident at any length.
pub fn looks_like_utf8(bytes: &[u8]) -> bool {
    !bytes.is_ascii() && std::str::from_utf8(bytes).is_ok()
}

/// The 1-based column the editor shows for the character after `line_prefix`.
/// Monaco counts UTF-16 code units, so a character outside the Basic
/// Multilingual Plane is two columns.
fn editor_column(line_prefix: &str) -> usize {
    line_prefix.encode_utf16().count() + 1
}

impl std::error::Error for DecodeRefused {}

/// A save refused because the text holds a character the target encoding has
/// no bytes for. Nothing is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unmappable {
    pub encoding: &'static Encoding,
    pub character: char,
    /// 1-based line of the character in the text being saved.
    pub line: usize,
    /// 1-based column as the editor shows it (UTF-16 code units).
    pub column: usize,
}

impl std::fmt::Display for Unmappable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "refusing to save: the character \u{201c}{}\u{201d} (U+{:04X}) on line {}, column {} \
             cannot be written in {}; nothing was written. Remove it, or save the file as UTF-8 \
             instead",
            self.character,
            u32::from(self.character),
            self.line,
            self.column,
            self.encoding.name()
        )
    }
}

impl std::error::Error for Unmappable {}

/// Resolve a client's encoding name to an encoding the editor can round-trip.
///
/// Any WHATWG label is accepted, so `iso-8859-1` names windows-1252 exactly as
/// a browser would. The `replacement` encoding is refused: it decodes every
/// file to a single replacement character and was made to neutralise content,
/// not to edit it.
pub fn encoding_for_label(label: &str) -> anyhow::Result<&'static Encoding> {
    match Encoding::for_label(label.trim().as_bytes()) {
        Some(encoding) if encoding != encoding_rs::REPLACEMENT => Ok(encoding),
        _ => anyhow::bail!("unknown text encoding: {label}"),
    }
}

/// Whether `encoding` has a byte-order mark at all.
pub fn has_bom(encoding: &'static Encoding) -> bool {
    encoding == UTF_8 || encoding == UTF_16LE || encoding == UTF_16BE
}

fn bom_bytes(encoding: &'static Encoding) -> &'static [u8] {
    if encoding == UTF_8 {
        b"\xEF\xBB\xBF"
    } else if encoding == UTF_16LE {
        b"\xFF\xFE"
    } else if encoding == UTF_16BE {
        b"\xFE\xFF"
    } else {
        b""
    }
}

/// Decode a file's bytes for the editor.
///
/// With no `requested` encoding the rules are, in order: a byte-order mark
/// names the encoding (and a mark whose bytes do not decode is ignored, so
/// the rest of the rules apply to the whole file, mark included); valid UTF-8
/// that sniffs as text is UTF-8; a NUL or a binary signature is binary; the
/// rest is windows-1252, flagged as a fallback.
///
/// The NUL test runs on the DECODED text after a mark, because UTF-16 is full
/// of zero bytes by design, and on the raw bytes otherwise, where a zero byte
/// in an ASCII-compatible encoding is a NUL character.
///
/// With `requested`, exactly that encoding is used. A leading mark for that
/// same encoding is set aside and remembered; any other leading bytes are part
/// of the text. Bytes that do not decode are refused. Anything that does decode
/// is text, however garbled: that is the person's call, and nothing is lost.
///
/// Every text answer is then checked against the one promise a save makes:
/// encoding the text back gives exactly the file's bytes. Where it does not
/// (see [`Unwritable`]), the text still opens, but `unwritable` says why it
/// must not be saved in this encoding.
pub fn decode(
    bytes: &[u8],
    requested: Option<&'static Encoding>,
) -> Result<Decoded, DecodeRefused> {
    let decoded = match requested {
        Some(encoding) => Decoded::Text(decode_exactly(bytes, encoding)?),
        None => decode_auto(bytes),
    };
    Ok(match decoded {
        Decoded::Text(mut t) => {
            t.unwritable = unwritable(bytes, &t);
            Decoded::Text(t)
        }
        Decoded::Binary => Decoded::Binary,
    })
}

fn unwritable(bytes: &[u8], t: &DecodedText) -> Option<Unwritable> {
    if t.text.starts_with('\u{FEFF}') {
        return Some(if t.bom {
            Unwritable::DoubledMark
        } else {
            Unwritable::LeadingMark
        });
    }
    match encode(&t.text, t.encoding, t.bom) {
        Ok(back) if back == bytes => None,
        _ => Some(Unwritable::RoundTrip),
    }
}

fn decode_auto(bytes: &[u8]) -> Decoded {
    if let Some((encoding, bom_len)) = Encoding::for_bom(bytes)
        && let Some(text) = strict_decode(&bytes[bom_len..], encoding)
        && !text.contains('\0')
    {
        return Decoded::Text(DecodedText {
            text,
            encoding,
            bom: true,
            fallback: false,
            unwritable: None,
        });
    }
    if crate::diff::is_renderable_text(bytes) {
        return Decoded::Text(DecodedText {
            // `is_renderable_text` has just proved the bytes are UTF-8.
            text: String::from_utf8_lossy(bytes).into_owned(),
            encoding: UTF_8,
            bom: false,
            fallback: false,
            unwritable: None,
        });
    }
    if bytes.contains(&0)
        || content_inspector::inspect(bytes) == content_inspector::ContentType::BINARY
    {
        return Decoded::Binary;
    }
    match strict_decode(bytes, WINDOWS_1252) {
        Some(text) => Decoded::Text(DecodedText {
            text,
            encoding: WINDOWS_1252,
            bom: false,
            fallback: true,
            unwritable: None,
        }),
        // windows-1252 assigns every byte, so this is unreachable in practice;
        // binary is the answer that can never lead to a lossy save.
        None => Decoded::Binary,
    }
}

fn decode_exactly(bytes: &[u8], encoding: &'static Encoding) -> Result<DecodedText, DecodeRefused> {
    let mark = bom_bytes(encoding);
    let bom = !mark.is_empty() && bytes.starts_with(mark);
    let body = if bom { &bytes[mark.len()..] } else { bytes };
    let start = bytes.len() - body.len();
    let text = decode_reporting(body, encoding).map_err(|offset| DecodeRefused {
        encoding,
        offset: start + offset,
    })?;
    Ok(DecodedText {
        text,
        encoding,
        bom,
        fallback: false,
        unwritable: None,
    })
}

fn strict_decode(bytes: &[u8], encoding: &'static Encoding) -> Option<String> {
    decode_reporting(bytes, encoding).ok()
}

/// Decode without replacement and without BOM handling, answering the byte
/// offset of the first malformed sequence on failure.
fn decode_reporting(bytes: &[u8], encoding: &'static Encoding) -> Result<String, usize> {
    let mut decoder = encoding.new_decoder_without_bom_handling();
    let capacity = decoder
        .max_utf8_buffer_length_without_replacement(bytes.len())
        .unwrap_or(bytes.len().saturating_mul(3));
    let mut text = String::with_capacity(capacity);
    let (result, read) = decoder.decode_to_string_without_replacement(bytes, &mut text, true);
    match result {
        DecoderResult::InputEmpty => Ok(text),
        DecoderResult::Malformed(bad, extra) => {
            Err(read.saturating_sub(usize::from(bad) + usize::from(extra)))
        }
        // The buffer was sized by the decoder's own worst case, so this means
        // the input could not be decoded in one pass; call it malformed at the
        // point decoding stopped rather than return a truncated text.
        DecoderResult::OutputFull => Err(read),
    }
}

/// Encode editor text into `encoding`, with the byte-order mark when `bom`.
///
/// UTF-16 is encoded by hand: `encoding_rs`'s encoder writes UTF-8 for it, the
/// way a browser submits a form, which is right for the web and wrong for a
/// file. Line endings are whatever the text holds; nothing is normalised.
pub fn encode(text: &str, encoding: &'static Encoding, bom: bool) -> anyhow::Result<Vec<u8>> {
    if bom && !has_bom(encoding) {
        anyhow::bail!(
            "{} has no byte-order mark, so a save cannot write one",
            encoding.name()
        );
    }
    if has_bom(encoding) && text.starts_with('\u{feff}') {
        return Err(anyhow::Error::new(LeadingMarkInText {
            encoding,
            from_file: false,
        }));
    }
    let mut out = Vec::with_capacity(text.len() + 3);
    if bom {
        out.extend_from_slice(bom_bytes(encoding));
    }
    if encoding == UTF_8 {
        out.extend_from_slice(text.as_bytes());
        return Ok(out);
    }
    if encoding == UTF_16LE || encoding == UTF_16BE {
        let little = encoding == UTF_16LE;
        out.reserve(text.len() * 2);
        for unit in text.encode_utf16() {
            out.extend_from_slice(&if little {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        return Ok(out);
    }
    let mut encoder = encoding.new_encoder();
    let mut rest = text;
    loop {
        let needed = encoder
            .max_buffer_length_from_utf8_without_replacement(rest.len())
            .unwrap_or(rest.len().saturating_mul(4))
            .max(16);
        out.reserve(needed);
        let (result, read) =
            encoder.encode_from_utf8_to_vec_without_replacement(rest, &mut out, true);
        match result {
            EncoderResult::InputEmpty => {
                // Some encoders write a character they have no exact bytes for
                // as a near one (Shift_JIS writes the yen sign as 5C, which
                // reads back as a backslash). That is a success to the encoder
                // and a different text on disk, so the bytes are read back and
                // must give exactly what was submitted.
                let back = decode_reporting(&out, encoding).unwrap_or_default();
                if back == text {
                    return Ok(out);
                }
                let offset = text
                    .char_indices()
                    .zip(back.chars().map(Some).chain(std::iter::repeat(None)))
                    .find(|((_, a), b)| Some(*a) != *b)
                    .map_or(0, |((i, _), _)| i);
                let character = text[offset..].chars().next().unwrap_or('\u{fffd}');
                return Err(unmappable_at(text, offset, character, encoding));
            }
            EncoderResult::OutputFull => rest = &rest[read..],
            EncoderResult::Unmappable(character) => {
                // `read` counts the unmappable character itself.
                let offset = text.len() - rest.len() + read - character.len_utf8();
                return Err(unmappable_at(text, offset, character, encoding));
            }
        }
    }
}

/// The refusal for `character` at byte `offset` of `text`, placed the way the
/// editor shows it.
fn unmappable_at(
    text: &str,
    offset: usize,
    character: char,
    encoding: &'static Encoding,
) -> anyhow::Error {
    let before = &text[..offset];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    anyhow::Error::new(Unmappable {
        encoding,
        character,
        line,
        column: editor_column(&before[line_start..]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(decoded: Result<Decoded, DecodeRefused>) -> DecodedText {
        match decoded.expect("decodes") {
            Decoded::Text(t) => t,
            Decoded::Binary => panic!("expected text, got binary"),
        }
    }

    #[test]
    fn utf8_without_a_mark_is_plain_utf8() {
        let t = text(decode("caf\u{e9}\n".as_bytes(), None));
        assert_eq!(t.text, "caf\u{e9}\n");
        assert_eq!(t.encoding, UTF_8);
        assert!(!t.bom && !t.fallback);
    }

    #[test]
    fn an_empty_file_is_plain_utf8() {
        let t = text(decode(b"", None));
        assert_eq!(t.encoding, UTF_8);
        assert_eq!(t.text, "");
    }

    #[test]
    fn a_utf8_mark_is_set_aside_and_remembered() {
        let t = text(decode(b"\xEF\xBB\xBFhello\n", None));
        assert_eq!(t.text, "hello\n");
        assert_eq!(t.encoding, UTF_8);
        assert!(t.bom && !t.fallback);
    }

    #[test]
    fn utf16_with_a_mark_opens_in_both_byte_orders() {
        let le = [0xFF, 0xFE, b'h', 0, b'i', 0, b'\n', 0];
        let t = text(decode(&le, None));
        assert_eq!(
            (t.text.as_str(), t.encoding, t.bom),
            ("hi\n", UTF_16LE, true)
        );
        let be = [0xFE, 0xFF, 0, b'h', 0, b'i', 0, b'\n'];
        let t = text(decode(&be, None));
        assert_eq!(
            (t.text.as_str(), t.encoding, t.bom),
            ("hi\n", UTF_16BE, true)
        );
    }

    #[test]
    fn bytes_that_are_not_utf8_fall_back_to_windows_1252_and_say_so() {
        let t = text(decode(b"caf\xe9 \x80\n", None));
        assert_eq!(t.text, "caf\u{e9} \u{20ac}\n");
        assert_eq!(t.encoding, WINDOWS_1252);
        assert!(t.fallback && !t.bom);
    }

    #[test]
    fn a_nul_without_a_mark_is_binary_however_late_it_comes() {
        let mut late = b"caf\xe9".to_vec();
        late.extend(std::iter::repeat_n(b'a', 4096));
        late.push(0);
        assert_eq!(decode(&late, None).unwrap(), Decoded::Binary);
        assert_eq!(decode(b"valid\0utf8", None).unwrap(), Decoded::Binary);
    }

    #[test]
    fn a_binary_signature_is_binary_even_without_a_nul() {
        // A PNG header has no NUL in its first bytes here and is not UTF-8.
        assert_eq!(
            decode(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a], None).unwrap(),
            Decoded::Binary
        );
    }

    #[test]
    fn a_utf16_mark_on_bytes_that_decode_to_nuls_is_binary() {
        // A binary that happens to start with FF FE: four zero bytes in a row
        // decode to U+0000 twice, which is no text file.
        let bytes = [0xFF, 0xFE, 0, 0, 0, 0, 0x12, 0x34];
        assert_eq!(decode(&bytes, None).unwrap(), Decoded::Binary);
    }

    #[test]
    fn a_requested_encoding_is_used_exactly() {
        // Shift_JIS for "nihongo", which the automatic rules would call windows-1252.
        let sjis = [0x93, 0xfa, 0x96, 0x7b, 0x8c, 0xea];
        assert!(text(decode(&sjis, None)).fallback);
        let t = text(decode(&sjis, Some(encoding_rs::SHIFT_JIS)));
        assert_eq!(t.text, "\u{65e5}\u{672c}\u{8a9e}");
        assert_eq!(t.encoding, encoding_rs::SHIFT_JIS);
        assert!(!t.fallback && !t.bom);
    }

    #[test]
    fn a_requested_encoding_the_bytes_are_not_is_refused_with_where() {
        let refused = decode(b"ok \xff\xfe more", Some(UTF_8)).unwrap_err();
        assert_eq!(refused.encoding, UTF_8);
        assert_eq!(refused.offset, 3);
        assert!(refused.to_string().contains("not valid UTF-8"));
        // An odd byte count is no UTF-16.
        assert!(decode(b"\xFF\xFEa\x00b", Some(UTF_16LE)).is_err());
    }

    #[test]
    fn a_requested_encoding_that_reads_a_nul_is_still_text() {
        // Garbled but lossless is the person's call: UTF-16 read as
        // windows-1252 is text full of NULs, and it writes back unchanged.
        let utf16 = [0xFF, 0xFE, b'h', 0, b'i', 0];
        let t = text(decode(&utf16, Some(WINDOWS_1252)));
        assert_eq!(t.text, "\u{ff}\u{fe}h\0i\0");
        assert_eq!(t.unwritable, None);
    }

    /// `encoding_rs` decodes some sequences to a character it encodes back as
    /// different bytes (NEC row 13 "≒" 87 90 comes back as 81 E0), and some it
    /// cannot encode at all (the user-defined rows from F0). A save of such a
    /// file would change bytes nobody touched, so it opens read-only.
    #[test]
    fn a_decode_that_would_not_write_back_the_same_bytes_is_not_writable() {
        for bytes in [
            &[0x87, 0x90, b'\n'][..],
            &[0xF0, 0x40, b'\n'][..],
            &[0xED, 0x40, b'\n'][..],
        ] {
            let t = text(decode(bytes, Some(encoding_rs::SHIFT_JIS)));
            assert_eq!(t.unwritable, Some(Unwritable::RoundTrip), "{bytes:x?}");
        }
        // Half-width katakana in ISO-2022-JP come back full-width.
        let katakana = b"\x1b(I1\x1b(B\n";
        let t = text(decode(katakana, Some(encoding_rs::ISO_2022_JP)));
        assert_eq!(t.unwritable, Some(Unwritable::RoundTrip));
        // Big5 and gb18030 have their own.
        let t = text(decode(&[0x8E, 0x69], Some(encoding_rs::BIG5)));
        assert_eq!(t.unwritable, Some(Unwritable::RoundTrip));
        let t = text(decode(&[0x80], Some(encoding_rs::GB18030)));
        assert_eq!(t.unwritable, Some(Unwritable::RoundTrip));
    }

    #[test]
    fn the_ordinary_cases_are_writable() {
        for (bytes, requested) in [
            (&b"caf\xe9\n"[..], None),
            ("caf\u{e9}\n".as_bytes(), None),
            (&b"\xEF\xBB\xBFhi"[..], None),
            (&[0xFF, 0xFE, b'h', 0][..], None),
            (&[0x93, 0xfa, 0x96, 0x7b][..], Some(encoding_rs::SHIFT_JIS)),
            ("caf\u{e9}".as_bytes(), Some(WINDOWS_1252)),
        ] {
            assert_eq!(
                text(decode(bytes, requested)).unwritable,
                None,
                "{bytes:x?}"
            );
        }
    }

    /// The editor drops a leading U+FEFF from its text, so a file with two marks
    /// would lose one on its first save.
    #[test]
    fn a_doubled_mark_opens_but_is_not_writable() {
        let t = text(decode(b"\xEF\xBB\xBF\xEF\xBB\xBFhi", None));
        assert!(t.bom);
        assert_eq!(t.unwritable, Some(Unwritable::DoubledMark));
        let t = text(decode(&[0xFF, 0xFE, 0xFF, 0xFE, b'h', 0], None));
        assert_eq!(t.unwritable, Some(Unwritable::DoubledMark));
        let t = text(decode(b"\xEF\xBB\xBF\xEF\xBB\xBFhi", Some(UTF_8)));
        assert_eq!(t.unwritable, Some(Unwritable::DoubledMark));
    }

    /// gb18030 spells U+FEFF as 84 31 95 33 and has no mark of its own, so a
    /// text starting with it is not "two marks": the reason says what it is.
    #[test]
    fn a_leading_mark_character_with_no_mark_before_it_has_its_own_reason() {
        let t = text(decode(
            &[0x84, 0x31, 0x95, 0x33, b'h'],
            Some(encoding_rs::GB18030),
        ));
        assert!(!t.bom);
        assert_eq!(t.unwritable, Some(Unwritable::LeadingMark));
        assert_eq!(Unwritable::LeadingMark.as_str(), "leading_mark");
    }

    #[test]
    fn line_endings_are_uniform_or_mixed_and_a_save_writes_monacos_pick() {
        let le = |s: &str| line_endings(s);
        assert_eq!(
            le("a\nb\n"),
            LineEndings {
                mixed: false,
                saves_as: Eol::Lf
            }
        );
        assert_eq!(
            le("a\r\nb\r\n"),
            LineEndings {
                mixed: false,
                saves_as: Eol::Crlf
            }
        );
        assert_eq!(
            le("no break"),
            LineEndings {
                mixed: false,
                saves_as: Eol::Lf
            }
        );
        // Mixed: the majority wins, a tie goes to LF.
        assert_eq!(
            le("a\r\nb\r\nc\n"),
            LineEndings {
                mixed: true,
                saves_as: Eol::Crlf
            }
        );
        assert_eq!(
            le("a\r\nb\n"),
            LineEndings {
                mixed: true,
                saves_as: Eol::Lf
            }
        );
        // A lone CR is a line break of its own and counts with CRLF.
        assert_eq!(
            le("a\rb\rc\n"),
            LineEndings {
                mixed: true,
                saves_as: Eol::Crlf
            }
        );
        assert_eq!(
            le("a\rb"),
            LineEndings {
                mixed: true,
                saves_as: Eol::Crlf
            }
        );
    }

    /// The editor counts columns in UTF-16 units, so an astral character before
    /// the refused one on its line counts twice.
    #[test]
    fn a_refusal_reports_the_column_the_editor_shows() {
        assert_eq!(editor_column("ab"), 3);
        assert_eq!(editor_column("\u{1F600}a"), 4);
    }

    #[test]
    fn a_requested_encoding_sets_aside_only_its_own_mark() {
        let t = text(decode(b"\xEF\xBB\xBFhi", Some(UTF_8)));
        assert_eq!((t.text.as_str(), t.bom), ("hi", true));
        // Asked for windows-1252, a UTF-8 mark is three characters of text.
        let t = text(decode(b"\xEF\xBB\xBFhi", Some(WINDOWS_1252)));
        assert_eq!((t.text.as_str(), t.bom), ("\u{ef}\u{bb}\u{bf}hi", false));
        // UTF-16 asked for without a mark in the file reads the whole file.
        let t = text(decode(&[b'h', 0, b'i', 0], Some(UTF_16LE)));
        assert_eq!((t.text.as_str(), t.bom), ("hi", false));
    }

    #[test]
    fn latin1_round_trips_byte_for_byte() {
        let mut every: Vec<u8> = (0x20..=0xFF).collect();
        every.extend_from_slice(b"\r\nline two\r\n");
        let t = text(decode(&every, None));
        assert!(t.fallback);
        assert_eq!(encode(&t.text, t.encoding, t.bom).unwrap(), every);
    }

    #[test]
    fn utf16_with_a_mark_round_trips_byte_for_byte_in_both_orders() {
        for (encoding, bom) in [(UTF_16LE, [0xFF, 0xFE]), (UTF_16BE, [0xFE, 0xFF])] {
            let content = "caf\u{e9} \u{1f600}\r\nzwei\r\n";
            let mut bytes = bom.to_vec();
            for unit in content.encode_utf16() {
                bytes.extend_from_slice(&if encoding == UTF_16LE {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            let t = text(decode(&bytes, None));
            assert_eq!(t.encoding, encoding);
            assert_eq!(t.text, content);
            assert_eq!(encode(&t.text, t.encoding, t.bom).unwrap(), bytes);
        }
    }

    #[test]
    fn utf8_with_a_mark_round_trips_and_crlf_is_kept() {
        let bytes = b"\xEF\xBB\xBFone\r\ntwo\r\n";
        let t = text(decode(bytes, None));
        assert_eq!(encode(&t.text, t.encoding, t.bom).unwrap(), bytes);
        assert_eq!(
            encode("a\r\nb\n", WINDOWS_1252, false).unwrap(),
            b"a\r\nb\n"
        );
    }

    #[test]
    fn shift_jis_round_trips() {
        let sjis = [0x93, 0xfa, 0x96, 0x7b, 0x8c, 0xea, b'\n'];
        let t = text(decode(&sjis, Some(encoding_rs::SHIFT_JIS)));
        assert_eq!(encode(&t.text, t.encoding, t.bom).unwrap(), sjis);
    }

    #[test]
    fn a_best_fit_character_that_would_come_back_as_another_is_refused() {
        // Shift_JIS writes the yen sign as 5C, which reads back as a backslash:
        // the save would succeed and the file would hold a different text.
        let err = encode("ok\n\u{a5}1\n", encoding_rs::SHIFT_JIS, false).unwrap_err();
        let refused = err.downcast_ref::<Unmappable>().expect("typed refusal");
        assert_eq!(refused.character, '\u{a5}');
        assert_eq!((refused.line, refused.column), (2, 1));
        // The overline comes back as a tilde.
        let err = encode("\u{203e}", encoding_rs::SHIFT_JIS, false).unwrap_err();
        assert_eq!(
            err.downcast_ref::<Unmappable>().unwrap().character,
            '\u{203e}'
        );
        // EUC-JP has the same near fit; ISO-2022-JP writes the yen sign
        // through its JIS-Roman escape, which reads back as the yen sign.
        assert!(encode("\u{a5}", encoding_rs::EUC_JP, false).is_err());
        let iso = encode("\u{a5}", encoding_rs::ISO_2022_JP, false).unwrap();
        assert_eq!(
            decode_reporting(&iso, encoding_rs::ISO_2022_JP).unwrap(),
            "\u{a5}"
        );
    }

    /// The single-byte code pages and gb18030 write back exactly what they
    /// read, so the check never refuses an honest save in them.
    #[test]
    fn a_text_starting_with_a_mark_character_is_refused_in_utf8_and_utf16() {
        // In UTF-8 and UTF-16 a leading U+FEFF is the bytes of a mark: a read
        // would take it as one, so the text and the file would disagree. The
        // mark is a choice of its own, never a character in the text.
        for (encoding, bom) in [
            (UTF_8, false),
            (UTF_8, true),
            (UTF_16LE, true),
            (UTF_16BE, false),
        ] {
            let err = encode("\u{feff}a", encoding, bom).unwrap_err();
            let refused = err
                .downcast_ref::<LeadingMarkInText>()
                .expect("typed refusal");
            assert_eq!(refused.encoding, encoding);
            assert!(err.to_string().contains("nothing was written"));
        }
        // Anywhere else in the text it is an ordinary character.
        assert!(encode("a\u{feff}", UTF_8, false).is_ok());
        // An encoding with no mark writes it as its own bytes.
        assert!(encode("\u{feff}a", encoding_rs::GB18030, false).is_ok());
    }

    #[test]
    fn exact_encodings_are_not_refused_by_the_read_back_check() {
        let latin = "caf\u{e9} \u{20ac} na\u{ef}ve\n";
        assert!(encode(latin, WINDOWS_1252, false).is_ok());
        assert!(encode("\u{416}\u{436}", encoding_rs::WINDOWS_1251, false).is_ok());
        assert!(
            encode(
                "\u{65e5}\u{672c} \u{1f600} \u{a5}",
                encoding_rs::GB18030,
                false
            )
            .is_ok()
        );
        assert!(encode("\u{65e5}\u{672c}\u{8a9e}", encoding_rs::SHIFT_JIS, false).is_ok());
    }

    #[test]
    fn a_character_the_encoding_cannot_hold_is_refused_with_where() {
        let err = encode("caf\u{e9}\nok \u{65e5}\n", WINDOWS_1252, false).unwrap_err();
        let unmappable = err.downcast_ref::<Unmappable>().expect("typed refusal");
        assert_eq!(unmappable.character, '\u{65e5}');
        assert_eq!((unmappable.line, unmappable.column), (2, 4));
        assert!(err.to_string().contains("U+65E5"));
        assert!(err.to_string().contains("nothing was written"));
    }

    #[test]
    fn an_unmappable_character_deep_in_a_long_text_is_still_found() {
        let mut long = "a".repeat(100_000);
        long.push('\u{2603}');
        let err = encode(&long, encoding_rs::ISO_8859_15, false).unwrap_err();
        let unmappable = err.downcast_ref::<Unmappable>().unwrap();
        assert_eq!((unmappable.line, unmappable.column), (1, 100_001));
    }

    #[test]
    fn a_mark_is_refused_for_an_encoding_that_has_none() {
        assert!(encode("x", WINDOWS_1252, true).is_err());
    }

    /// The browser's encoding menus (`EDITOR_ENCODINGS` in the web's
    /// lib/editorEncoding.ts) send these names and compare them against what a
    /// read reports, so each must be the canonical name this side answers with.
    #[test]
    fn every_name_the_web_menus_offer_is_canonical_here() {
        let source = include_str!("../../dux-web/web/src/lib/editorEncoding.ts");
        let names: Vec<&str> = source
            .lines()
            .filter_map(|line| line.trim().strip_prefix("{ name: \""))
            .filter_map(|rest| rest.split('"').next())
            .collect();
        assert!(
            names.len() > 20,
            "the list moved; update this test: {names:?}"
        );
        for name in names {
            let encoding = encoding_for_label(name).unwrap_or_else(|_| panic!("{name}"));
            assert_eq!(encoding.name(), name);
        }
    }

    #[test]
    fn labels_resolve_the_whatwg_way_and_replacement_is_refused() {
        assert_eq!(encoding_for_label("iso-8859-1").unwrap(), WINDOWS_1252);
        assert_eq!(encoding_for_label("UTF-16LE").unwrap(), UTF_16LE);
        assert_eq!(
            encoding_for_label("Shift_JIS").unwrap(),
            encoding_rs::SHIFT_JIS
        );
        assert!(encoding_for_label("replacement").is_err());
        assert!(encoding_for_label("iso-2022-kr").is_err());
        assert!(encoding_for_label("klingon").is_err());
    }
}
