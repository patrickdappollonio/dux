//! What the delete-agent dialog says when git could not count the commits its
//! branch box would destroy, built once as prose for both surfaces.
//!
//! The terminal UI renders it in its Confirm Delete Agent dialog, and the
//! browser's `lib/deleteAgent.ts` builds the same segments for
//! `DeleteSessionDialog`. The two are pinned against each other by
//! `tests/fixtures/prose_cross_language.json`.

use crate::prose::Prose;

/// The sentence saying the unpushed commits on `branches` could not be
/// counted, with git's `reason`. Every branch is a name; the reason is relayed
/// error text and stays plain. Two branches (a drifted agent) take the plural.
pub fn unpushed_count_failure_prose(branches: &[&str], reason: &str) -> Prose {
    let mut prose = Prose::new().text("dux could not count the unpushed commits on ");
    for (index, branch) in branches.iter().enumerate() {
        if index > 0 {
            prose.push_text(" and ");
        }
        prose.push_name(*branch);
    }
    prose.push_text(format!(" ({reason}); "));
    prose.text(if branches.len() > 1 {
        "check them before deleting them."
    } else {
        "check it before deleting it."
    })
}
