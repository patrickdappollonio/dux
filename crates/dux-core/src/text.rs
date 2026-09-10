//! Small text helpers shared by the engine and both surfaces.
//!
//! Counting nouns is the one piece of English every surface hand-rolls, and a
//! hand-rolled copy is where "1 files" and "1 process" come from. These are the
//! Rust twins of the web's `lib/formatRegularCount.ts`.

/// `"0 commits"` / `"1 commit"` / `"3 commits"`: count a regular noun that
/// pluralizes with a trailing `s`.
pub fn count_of(n: usize, singular: &str) -> String {
    count_of_with(n, singular, &format!("{singular}s"))
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
    fn count_of_with_uses_the_spelled_out_plural() {
        assert_eq!(count_of_with(0, "process", "processes"), "0 processes");
        assert_eq!(count_of_with(1, "process", "processes"), "1 process");
        assert_eq!(count_of_with(2, "process", "processes"), "2 processes");
    }
}
