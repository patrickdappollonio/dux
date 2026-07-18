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
