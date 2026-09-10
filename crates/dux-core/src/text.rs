//! Small text helpers shared by the engine and both surfaces.
//!
//! Counting nouns is the one piece of English every surface hand-rolls, and a
//! hand-rolled copy is where "1 files" and "1 process" come from. These are the
//! Rust twins of the web's `lib/formatRegularCount.ts`.

/// `"0 commits"` / `"1 commit"` / `"3 commits"`: count a regular noun that
/// pluralizes with a trailing `s`.
pub fn count_of(n: usize, singular: &str) -> String {
    if n == 1 {
        format!("{n} {singular}")
    } else {
        format!("{n} {singular}s")
    }
}

/// `"12,345"`: a decimal integer with a comma every three digits.
///
/// For counts a reader has to take in at a glance. The browser's half of the
/// same sentence gets this from `toLocaleString("en-US")`, so the separator is
/// a comma here too rather than the reader's own locale: the two strings have
/// to stay identical.
pub fn group_digits(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// `"1 process"` / `"2 processes"`: count a noun whose plural is spelled out
/// rather than derived.
pub fn count_of_with(n: usize, singular: &str, plural: &str) -> String {
    if n == 1 {
        format!("{n} {singular}")
    } else {
        format!("{n} {plural}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_of_pluralizes_a_regular_noun() {
        assert_eq!(count_of(0, "commit"), "0 commits");
        assert_eq!(count_of(1, "commit"), "1 commit");
        assert_eq!(count_of(3, "commit"), "3 commits");
    }

    #[test]
    fn group_digits_puts_a_comma_every_three_digits() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(7), "7");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1_000), "1,000");
        assert_eq!(group_digits(4_000), "4,000");
        assert_eq!(group_digits(12_345), "12,345");
        assert_eq!(group_digits(100_000), "100,000");
        assert_eq!(group_digits(2_000_000), "2,000,000");
    }

    #[test]
    fn count_of_with_uses_the_spelled_out_plural() {
        assert_eq!(count_of_with(0, "process", "processes"), "0 processes");
        assert_eq!(count_of_with(1, "process", "processes"), "1 process");
        assert_eq!(count_of_with(2, "process", "processes"), "2 processes");
    }
}
