//! A small tokenized relevance scorer over [`PALETTE_COMMANDS`].
//!
//! ## Why this exists
//!
//! The palette's own matching (the direct tier of
//! `RuntimeBindings::palette_matches`) is a contiguous phrase match: the
//! query, with whitespace and dashes collapsed to one `-`, must appear as a
//! substring of the command name or, failing that, of its description. That is
//! precise and predictable, and it is the tier the user sees first. It also
//! answers nothing at all for `new tab`, `toggle banner` or `kill agent`,
//! because those words are real but not adjacent in that order.
//!
//! This module is the second tier: every query token must PREFIX-match some
//! token of the command, in any order, and the hits are ranked. Mostly that
//! catches multi-word queries, but not only: the two tiers tokenize
//! differently, so a single word can land here too (the apostrophe rule
//! below is one way in). It is a
//! deliberately small BM25-shaped rule rather than a crate: at 77 records the
//! ranking cost is irrelevant, and every candidate crate would still have
//! needed custom splitting, prefix-AND semantics, field weights, subtraction of
//! the first tier and a canonical tie-break, which is most of the code here.
//!
//! ## The rule
//!
//! * Tokens are runs of alphanumerics, lowercased, with `'` and `’` removed
//!   first so `agent's` tokenizes as one `agents` rather than `agent` plus a
//!   junk `s`. Lowercasing is per code point (`char::to_lowercase`): there is
//!   no case folding and no Unicode normalization, so `ß` and `ss` are
//!   different tokens. That is stated rather than fixed; the corpus is ASCII
//!   command names and English descriptions.
//! * Repeated query tokens are deduplicated, so `agent agent` scores exactly
//!   as `agent` does.
//! * A command matches only when EVERY query token prefixes at least one of
//!   its tokens. An empty or punctuation-only query matches nothing.
//! * A command's score is the sum, over query tokens, of
//!   `idf(token) * weight`, where `weight` is the best field hit that token
//!   got: an exact name token beats a name prefix beats an exact description
//!   token beats a description prefix.
//! * `idf(t) = ln((N + 1) / (df(t) + 1)) + 1`, scaled by 1000 and rounded to
//!   an integer so the sort key is a total order with no float in it. `df(t)`
//!   is PREFIX-AWARE: the number of commands carrying a token that starts with
//!   `t`, which is the number of documents the query token actually reaches.
//!   Using the literal token's own document frequency instead would hand a
//!   two-letter prefix like `ag` the rarest-token score while it matches half
//!   the table.
//! * Ties are broken by table order, so the ranking is fully deterministic.

use std::collections::HashSet;
use std::sync::OnceLock;

use super::PALETTE_COMMANDS;

/// Weight for a query token that equals a whole token of the command name.
const WEIGHT_NAME_EXACT: u32 = 300;
/// Weight for a query token that prefixes a token of the command name.
const WEIGHT_NAME_PREFIX: u32 = 200;
/// Weight for a query token that equals a whole token of the description.
const WEIGHT_DESCRIPTION_EXACT: u32 = 150;
/// Weight for a query token that prefixes a token of the description.
const WEIGHT_DESCRIPTION_PREFIX: u32 = 100;

/// Split `text` into lowercase alphanumeric tokens.
///
/// Apostrophes are removed rather than treated as separators; every other
/// non-alphanumeric character separates.
pub fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        if c == '\'' || c == '\u{2019}' {
            continue;
        }
        if c.is_alphanumeric() {
            current.extend(c.to_lowercase());
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Deduplicate query tokens, keeping first-seen order.
fn unique_tokens(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    tokenize(text)
        .into_iter()
        .filter(|token| seen.insert(token.clone()))
        .collect()
}

/// One command's tokenized name and description.
struct Document {
    name: Vec<String>,
    description: Vec<String>,
}

impl Document {
    /// The best weight this document offers for one query token, or `None`
    /// when the token prefixes nothing here.
    fn weight_for(&self, token: &str) -> Option<u32> {
        let mut best = None;
        for name_token in &self.name {
            if name_token == token {
                return Some(WEIGHT_NAME_EXACT);
            }
            if name_token.starts_with(token) {
                best = Some(WEIGHT_NAME_PREFIX);
            }
        }
        if best == Some(WEIGHT_NAME_PREFIX) {
            return best;
        }
        for description_token in &self.description {
            if description_token == token {
                best = Some(WEIGHT_DESCRIPTION_EXACT);
            } else if description_token.starts_with(token) && best != Some(WEIGHT_DESCRIPTION_EXACT)
            {
                best = Some(WEIGHT_DESCRIPTION_PREFIX);
            }
        }
        best
    }

    /// Whether any token of this document starts with `token`.
    fn contains_prefix(&self, token: &str) -> bool {
        self.name
            .iter()
            .chain(self.description.iter())
            .any(|t| t.starts_with(token))
    }
}

/// The tokenized palette, built once.
///
/// Only the tokenization is memoized: `idf` depends on the QUERY token,
/// because the document frequency it needs is prefix-aware, so there is no
/// per-token table to precompute. Scanning 77 tokenized records per query
/// token costs nothing at this size.
struct Corpus {
    documents: Vec<Document>,
}

impl Corpus {
    fn build<'a>(entries: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        Self {
            documents: entries
                .into_iter()
                .map(|(name, description)| Document {
                    name: tokenize(name),
                    description: tokenize(description),
                })
                .collect(),
        }
    }

    /// `ln((N + 1) / (df + 1)) + 1`, scaled by 1000 and rounded.
    fn idf_milli(&self, token: &str) -> u32 {
        let total = self.documents.len() as f64;
        let df = self
            .documents
            .iter()
            .filter(|document| document.contains_prefix(token))
            .count() as f64;
        let idf = ((total + 1.0) / (df + 1.0)).ln() + 1.0;
        (idf * 1000.0).round().max(0.0) as u32
    }

    /// Score one document against already-deduplicated query tokens, or `None`
    /// when the prefix-AND rule refuses it.
    fn score(&self, document: &Document, tokens: &[(String, u32)]) -> Option<u32> {
        let mut total: u32 = 0;
        for (token, idf) in tokens {
            let weight = document.weight_for(token)?;
            total = total.saturating_add(idf.saturating_mul(weight));
        }
        Some(total)
    }

    /// Every match, best first, ties in table order.
    fn ranked(&self, query: &str) -> Vec<ScoredCommand> {
        let tokens = unique_tokens(query)
            .into_iter()
            .map(|token| {
                let idf = self.idf_milli(&token);
                (token, idf)
            })
            .collect::<Vec<_>>();
        if tokens.is_empty() {
            return Vec::new();
        }
        let mut hits = self
            .documents
            .iter()
            .enumerate()
            .filter_map(|(index, document)| {
                self.score(document, &tokens)
                    .map(|score| ScoredCommand { index, score })
            })
            .collect::<Vec<_>>();
        hits.sort_by_key(|hit| (std::cmp::Reverse(hit.score), hit.index));
        hits
    }
}

fn palette_corpus() -> &'static Corpus {
    static CORPUS: OnceLock<Corpus> = OnceLock::new();
    CORPUS.get_or_init(|| {
        Corpus::build(
            PALETTE_COMMANDS
                .iter()
                .map(|command| (command.name, command.description)),
        )
    })
}

/// One ranked hit: an index into [`PALETTE_COMMANDS`] and its score.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScoredCommand {
    /// Index into [`PALETTE_COMMANDS`].
    pub index: usize,
    /// The summed score; comparable only within one query's results.
    pub score: u32,
}

/// Every palette command whose tokens are all prefixed by the query's tokens,
/// ranked best first with ties in table order.
pub fn ranked_matches(query: &str) -> Vec<ScoredCommand> {
    palette_corpus().ranked(query)
}

/// The names of [`ranked_matches`], in rank order. Convenience for callers
/// (and tests) that join by name rather than by index.
pub fn ranked_match_names(query: &str) -> Vec<&'static str> {
    ranked_matches(query)
        .into_iter()
        .map(|hit| PALETTE_COMMANDS[hit.index].name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer_splits_on_every_non_alphanumeric_and_lowercases() {
        assert_eq!(tokenize("new-agent-tab"), vec!["new", "agent", "tab"]);
        assert_eq!(tokenize("  New   Agent  "), vec!["new", "agent"]);
        assert_eq!(tokenize("open/current_pr!"), vec!["open", "current", "pr"]);
        assert_eq!(tokenize("v2 pane"), vec!["v2", "pane"]);
        assert!(tokenize("").is_empty());
        assert!(tokenize("---  ,.;").is_empty());
    }

    #[test]
    fn tokenizer_strips_apostrophes_instead_of_splitting_on_them() {
        assert_eq!(tokenize("agent's"), vec!["agents"]);
        assert_eq!(
            tokenize("agent\u{2019}s worktree"),
            vec!["agents", "worktree"]
        );
    }

    #[test]
    fn tokenizer_keeps_unicode_letters_and_lowercases_per_code_point() {
        assert_eq!(tokenize("Ärger Über"), vec!["ärger", "über"]);
        assert_eq!(tokenize("ÉCRAN"), vec!["écran"]);
        // Stated narrowly: code-point lowercasing only, no case folding, so
        // this stays a distinct token from `strasse`.
        assert_eq!(tokenize("Straße"), vec!["straße"]);
    }

    #[test]
    fn query_tokens_are_deduplicated() {
        assert_eq!(unique_tokens("agent AGENT agent tab"), vec!["agent", "tab"]);
        assert_eq!(
            ranked_match_names("agent agent"),
            ranked_match_names("agent")
        );
    }

    /// A tiny corpus so the weighting rules can be read off directly.
    fn fixture() -> Corpus {
        Corpus::build([
            ("alpha-widget", "a common thing"),                 // 0
            ("beta", "alpha lives in this common description"), // 1
            ("gamma", "alphabetical common ordering"),          // 2
            ("delta", "a common thing"),                        // 3
        ])
    }

    fn ranked_indices(corpus: &Corpus, query: &str) -> Vec<usize> {
        corpus.ranked(query).into_iter().map(|h| h.index).collect()
    }

    #[test]
    fn every_query_token_must_prefix_some_document_token() {
        let corpus = fixture();
        // `wid` prefixes `widget`; `zzz` prefixes nothing, so the AND fails.
        assert_eq!(ranked_indices(&corpus, "alpha wid"), vec![0]);
        assert!(ranked_indices(&corpus, "alpha zzz").is_empty());
        // Order does not matter.
        assert_eq!(
            ranked_indices(&corpus, "wid alpha"),
            ranked_indices(&corpus, "alpha wid")
        );
        // An empty and a punctuation-only query match nothing at all.
        assert!(ranked_indices(&corpus, "").is_empty());
        assert!(ranked_indices(&corpus, " -- .. ").is_empty());
    }

    #[test]
    fn a_name_hit_outranks_a_description_hit_and_exact_outranks_prefix() {
        let corpus = fixture();
        // 0 has `alpha` as a name token (exact name), 1 has it as an exact
        // description token, 2 only as a description PREFIX (`alphabetical`).
        assert_eq!(ranked_indices(&corpus, "alpha"), vec![0, 1, 2]);
        let scores = corpus.ranked("alpha");
        assert!(scores[0].score > scores[1].score);
        assert!(scores[1].score > scores[2].score);
        // `alph` is a prefix of the name token, which still beats both
        // description hits.
        assert_eq!(ranked_indices(&corpus, "alph"), vec![0, 1, 2]);
    }

    #[test]
    fn a_rare_token_carries_more_weight_than_a_common_one() {
        let corpus = fixture();
        // `common` appears in all four descriptions; `alpha` reaches three.
        assert!(corpus.idf_milli("alpha") > corpus.idf_milli("common"));
        // `widget` reaches one document, so it is the rarest of the three.
        assert!(corpus.idf_milli("widget") > corpus.idf_milli("alpha"));
    }

    #[test]
    fn document_frequency_is_prefix_aware() {
        let corpus = fixture();
        // The literal token `alpha` occurs in two documents, but the prefix
        // `alpha` REACHES three (`alphabetical`), so its idf must equal the
        // idf of any other prefix reaching the same three.
        assert_eq!(corpus.idf_milli("alpha"), corpus.idf_milli("alph"));
        assert!(corpus.idf_milli("alphab") > corpus.idf_milli("alpha"));
    }

    #[test]
    fn ties_break_by_table_order() {
        let corpus = fixture();
        // 0 and 3 have identical descriptions and no other hit, so `thing`
        // scores them the same and the table order decides.
        assert_eq!(ranked_indices(&corpus, "thing"), vec![0, 3]);
        assert_eq!(
            corpus.ranked("thing")[0].score,
            corpus.ranked("thing")[1].score
        );
    }

    // ── Corpus pins over the REAL command table ──────────────────────────
    //
    // These assert the EXACT ranked output, not merely the winner: a reorder
    // is a product change and should have to be written down here.

    #[test]
    fn corpus_new_tab() {
        assert_eq!(
            ranked_match_names("new tab"),
            vec!["new-agent-tab"],
            "only one command carries both words; `close-tab` has the tab but not the new"
        );
    }

    #[test]
    fn corpus_toggle_banner() {
        assert_eq!(
            ranked_match_names("toggle banner"),
            vec!["toggle-pr-banner-position"]
        );
    }

    #[test]
    fn corpus_kill_agent() {
        assert_eq!(ranked_match_names("kill agent"), vec!["kill-running"]);
    }

    #[test]
    fn corpus_open_pr() {
        // `pr` also prefixes `project` and `provider`, which is why the tail
        // is there at all; the exact-name hit leads by a wide margin.
        assert_eq!(
            ranked_match_names("open pr"),
            vec!["open-current-pr", "add-project", "new-terminal-for-project"]
        );
    }

    #[test]
    fn corpus_tail_finds_the_tailscale_command() {
        assert_eq!(ranked_match_names("tail"), vec!["set-tailscale-mode"]);
    }

    #[test]
    fn corpus_agent_tab_has_both_kinds_of_hit() {
        // Two name-token hits first, then the commands that carry one of the
        // words in their description only.
        assert_eq!(
            ranked_match_names("agent tab"),
            vec![
                "new-agent-tab",
                "toggle-tab-to-agent",
                "close-tab",
                "toggle-always-show-tabs"
            ]
        );
    }

    #[test]
    fn corpus_agent() {
        // The whole ranked list for the single commonest word in the table:
        // every command whose NAME carries it, in table order, then the ones
        // that only mention it in their description.
        assert_eq!(
            ranked_match_names("agent"),
            vec![
                "new-agent",
                "new-agent-from-pr",
                "new-agent-from-worktree",
                "fork-agent",
                "change-agent-provider",
                "new-agent-tab",
                "toggle-agent-auto-reopen",
                "rerun-startup-command-on-agent",
                "show-agent",
                "reconnect-agent",
                "delete-agent",
                "new-terminal-for-agent",
                "new-standalone-agent",
                "rename-agent",
                "agent-info",
                "toggle-tab-to-agent",
                "force-reconnect-agent",
                "move-agent-up",
                "move-agent-down",
                "move-agent-top",
                "move-agent-bottom",
                "filter-agents",
                "toggle-project-auto-reopen-agents",
                "sort-agents",
                "close-tab",
                "read-startup-command-logs",
                "attach-pull-request",
                "toggle-always-show-tabs",
                "toggle-randomized-pet-name-default",
                "toggle-pr-banner-position",
                "toggle-project",
                "change-default-provider",
                "change-project-default-provider",
                "start-web-server",
                "stop-background-server",
                "configure-project-env",
                "copy-path",
                "open-worktree",
                "open-worktree-with",
                "show-terminal",
                "open-current-pr",
                "detach-pull-request",
                "resume-pull-request-autodetection",
                "kill-running",
                "resource-monitor",
                "refresh-changes",
            ]
        );
    }

    #[test]
    fn corpus_prefix_matching_no_whole_token() {
        // `wor` is a whole token nowhere in the table; it reaches `worktree`
        // and `worktrees` by prefix only, name hits first.
        assert_eq!(
            ranked_match_names("wor"),
            vec![
                "new-agent-from-worktree",
                "manage-worktrees",
                "open-worktree",
                "open-worktree-with",
                "fork-agent",
                "change-agent-provider"
            ]
        );
    }

    #[test]
    fn corpus_duplicate_query_tokens_change_nothing() {
        assert_eq!(
            ranked_match_names("new new tab"),
            ranked_match_names("new tab")
        );
    }

    #[test]
    fn corpus_nonsense_query_matches_nothing() {
        assert!(ranked_match_names("qqzzx").is_empty());
        assert!(ranked_match_names("new qqzzx").is_empty());
    }
}
