//! Pure search matching for the flat agent/terminal list, the core-owned rule the
//! TUI's pane filter and the web's sidebar/hub search both apply so a query filters
//! identically on either surface. A filter that silently drops a row is a data-loss
//! bug, so this is small, pure, and unit-tested.
//!
//! Mirrors the web's `crates/dux-web/web/src/lib/agentSearch.ts` exactly; the tests
//! below share vectors with it. A query matches case-insensitively as a substring
//! against a small set of fields per row kind; an empty/whitespace query matches
//! everything (the list is shown unfiltered).

/// Normalize a raw query: trimmed and lowercased. An empty result means "match
/// everything".
pub fn normalize_query(query: &str) -> String {
    query.trim().to_lowercase()
}

fn haystack_has(query: &str, fields: &[Option<&str>]) -> bool {
    if query.is_empty() {
        return true;
    }
    fields
        .iter()
        .any(|field| field.unwrap_or("").to_lowercase().contains(query))
}

/// Match an agent row against a raw query. Fields: the display name (`title`,
/// falling back to `branch_name`), the project name, the branch, and every provider
/// the agent runs (its own provider plus each tab's provider, so a search for
/// "codex" finds an agent whose codex tab is the interesting one). `providers` is
/// the caller-gathered list (session provider first, then each tab's).
pub fn matches_session(
    title: Option<&str>,
    branch_name: &str,
    project_name: &str,
    providers: &[&str],
    query: &str,
) -> bool {
    let q = normalize_query(query);
    if q.is_empty() {
        return true;
    }
    if haystack_has(&q, &[title, Some(branch_name), Some(project_name)]) {
        return true;
    }
    providers.iter().any(|p| p.to_lowercase().contains(&q))
}

/// The CHAR range (start inclusive, end exclusive, in char indices, never
/// bytes) of the first case-insensitive occurrence of `query` in `field`, or
/// `None` when the query is empty/whitespace or does not occur. The search-hit
/// highlight uses this to emphasize the matched part of a row's name, so it
/// applies the exact same normalization the filter itself applies
/// (`normalize_query` + lowercase contains): what highlights is what matched.
///
/// Char indices, deliberately: user-visible labels carry multi-byte UTF-8
/// (CJK, emoji, box drawing), and byte-based slicing panics inside a
/// multi-byte char (the CLAUDE.md truncation rule). Lowercasing can EXPAND a
/// char (ß becomes ss), so the haystack is lowered char by char while
/// recording each lowered char's SOURCE char index; the range is then mapped
/// back through that record, keeping the highlight aligned with the original
/// string however the case-folding reshaped it.
pub fn match_char_range(field: &str, query: &str) -> Option<(usize, usize)> {
    let q: Vec<char> = normalize_query(query).chars().collect();
    if q.is_empty() {
        return None;
    }
    let mut lowered: Vec<char> = Vec::new();
    let mut source_index: Vec<usize> = Vec::new();
    for (index, ch) in field.chars().enumerate() {
        for lower in ch.to_lowercase() {
            lowered.push(lower);
            source_index.push(index);
        }
    }
    if q.len() > lowered.len() {
        return None;
    }
    for start in 0..=(lowered.len() - q.len()) {
        if lowered[start..start + q.len()] == q[..] {
            let from = source_index[start];
            let to = source_index[start + q.len() - 1] + 1;
            return Some((from, to));
        }
    }
    None
}

/// Match a terminal row against a raw query. Fields: the terminal's label and its
/// running foreground command, the owner label ("agent name" or "project"), and the
/// project name.
pub fn matches_terminal(
    label: &str,
    foreground_cmd: Option<&str>,
    owner_label: &str,
    project_name: &str,
    query: &str,
) -> bool {
    let q = normalize_query(query);
    haystack_has(
        &q,
        &[
            Some(label),
            foreground_cmd,
            Some(owner_label),
            Some(project_name),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_char_range_finds_a_case_insensitive_hit_in_char_indices() {
        assert_eq!(match_char_range("API-Refactor", "refactor"), Some((4, 12)));
        assert_eq!(match_char_range("feature/login", "LOGIN"), Some((8, 13)));
        assert_eq!(match_char_range("abc", "abc"), Some((0, 3)));
    }

    #[test]
    fn match_char_range_returns_none_for_no_hit_or_empty_query() {
        assert_eq!(match_char_range("api", "zzz"), None);
        assert_eq!(match_char_range("api", ""), None);
        assert_eq!(match_char_range("api", "   "), None);
    }

    #[test]
    fn match_char_range_counts_chars_not_bytes_for_multibyte_labels() {
        // "höhe-fix": the umlaut is two UTF-8 bytes but ONE char; "fix" starts
        // at char index 5, not byte index 6.
        assert_eq!(match_char_range("höhe-fix", "fix"), Some((5, 8)));
        // An emoji (single char here) before the hit shifts the range by one.
        assert_eq!(match_char_range("🦆 duck", "duck"), Some((2, 6)));
        // Matching THROUGH multi-byte chars works too.
        assert_eq!(match_char_range("日本語テスト", "語テ"), Some((2, 4)));
    }

    #[test]
    fn empty_query_matches_everything() {
        assert!(matches_session(
            Some("api"),
            "main",
            "proj",
            &["claude"],
            ""
        ));
        assert!(matches_session(None, "main", "proj", &[], "   "));
        assert!(matches_terminal("shell", None, "owner", "proj", ""));
    }

    #[test]
    fn matches_name_branch_and_project_case_insensitively() {
        // Title.
        assert!(matches_session(
            Some("API-Refactor"),
            "b",
            "proj",
            &[],
            "refactor"
        ));
        // Branch (name falls back to branch, but branch is matched directly too).
        assert!(matches_session(None, "feature/login", "proj", &[], "LOGIN"));
        // Project name.
        assert!(matches_session(Some("x"), "b", "demo-web", &[], "web"));
        // Non-match.
        assert!(!matches_session(Some("x"), "b", "proj", &["claude"], "zzz"));
    }

    #[test]
    fn matches_any_provider_including_tabs() {
        // The session runs claude, but a tab runs codex; searching "codex" hits.
        assert!(matches_session(
            Some("x"),
            "b",
            "proj",
            &["claude", "codex"],
            "codex"
        ));
        assert!(!matches_session(
            Some("x"),
            "b",
            "proj",
            &["claude"],
            "codex"
        ));
    }

    #[test]
    fn terminal_matches_label_command_owner_project() {
        assert!(matches_terminal(
            "Terminal 2",
            Some("npm run dev"),
            "api-agent",
            "proj",
            "dev"
        ));
        assert!(matches_terminal(
            "Terminal 2",
            None,
            "api-agent",
            "proj",
            "agent"
        ));
        assert!(!matches_terminal(
            "Terminal 2",
            None,
            "api-agent",
            "proj",
            "zzz"
        ));
    }
}
