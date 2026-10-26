//! POSIX shell quoting for text dux shows a user to paste into a shell.
//!
//! One rule, shared by every caller: wrap in single quotes, inside which
//! nothing is special, and spell an embedded apostrophe as `'\''` (close the
//! quote, an escaped apostrophe, reopen). Double quotes are not enough: `$`,
//! backticks and a closing `"` stay live inside them, so a crafted name would
//! run a command of its own. The web's `lib/shellQuote.ts` applies the same
//! rule.

/// `s` as ONE POSIX shell word, whatever it contains.
pub(crate) fn single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_plain_text_in_single_quotes() {
        assert_eq!(single_quote("feature-x"), "'feature-x'");
    }

    #[test]
    fn spells_an_apostrophe_by_leaving_and_reentering_the_quotes() {
        assert_eq!(single_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn leaves_double_quotes_and_dollars_inert() {
        assert_eq!(single_quote("a\"$b`c"), "'a\"$b`c'");
    }
}
