//! The quiet/inactive-tail search-expansion decision, core-owned and mirrored by
//! hand in the web's `quietTailForcedOpen` (`flatList.ts`), the two pinned by
//! shared test vectors.
//!
//! The tail auto-expands while a search matches something dormant, so the results
//! are visible; a user who collapses it while a matching query is active has made
//! an explicit call, so that dismissal wins until the query changes. The
//! dismissal is keyed on the normalized query (`agent_search::normalize_query`),
//! not the raw text, because the filter matches on the normalized query, so a
//! whitespace-only or case-only variant must not resurrect the tail.

/// Whether the quiet tail should render forced-open for the current search.
/// `normalized_query` is the trimmed/lowercased query (`None`/empty means no
/// active search). `dismissed_query` is the normalized query under which the user
/// last collapsed the search-expanded tail. `quiet_hit` is whether the query
/// matches at least one dormant row. Forces open when there is an active query
/// that hits a quiet row AND has not been dismissed for this exact normalized
/// query.
pub fn quiet_tail_forced_open(
    normalized_query: Option<&str>,
    dismissed_query: Option<&str>,
    quiet_hit: QuietHit,
) -> bool {
    let Some(query) = normalized_query.filter(|q| !q.is_empty()) else {
        return false;
    };
    if dismissed_query == Some(query) {
        return false;
    }
    quiet_hit == QuietHit::Found
}

/// Whether a search matches at least one dormant row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuietHit {
    /// At least one dormant row matches.
    Found,
    /// No dormant row matches.
    NotFound,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── SHARED VECTORS with flatList.test.ts `quietTailForcedOpen` ─────────────
    #[test]
    fn forces_open_only_on_an_undismissed_hitting_query() {
        // No active query: never forced open.
        assert!(!quiet_tail_forced_open(None, None, QuietHit::Found));
        assert!(!quiet_tail_forced_open(Some(""), None, QuietHit::Found));
        // Active query that hits a quiet row, not dismissed: forced open.
        assert!(quiet_tail_forced_open(Some("vim"), None, QuietHit::Found));
        // Active query but nothing quiet matches: not forced open.
        assert!(!quiet_tail_forced_open(
            Some("vim"),
            None,
            QuietHit::NotFound
        ));
        // Dismissed for this exact normalized query: stays closed even with a hit.
        assert!(!quiet_tail_forced_open(
            Some("vim"),
            Some("vim"),
            QuietHit::Found
        ));
        // Dismissed for a DIFFERENT query: the new query forces open again.
        assert!(quiet_tail_forced_open(
            Some("nvim"),
            Some("vim"),
            QuietHit::Found
        ));
    }

    #[test]
    fn a_normalized_variant_of_the_dismissed_query_does_not_resurrect_the_tail() {
        // The caller normalizes before calling, so "vim ", " VIM", etc. all arrive
        // as "vim" and match the dismissal. This vector documents that contract:
        // the same normalized query keeps the tail dismissed.
        assert!(!quiet_tail_forced_open(
            Some("vim"),
            Some("vim"),
            QuietHit::Found
        ));
    }
}
