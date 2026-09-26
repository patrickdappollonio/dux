//! A sentence that names things, kept as structure rather than as a finished
//! string.
//!
//! A dialog sentence mixes constant words with variables: a branch, a path, an
//! agent or project name, a provider. Both surfaces draw every such variable as
//! a chip (the terminal UI's themed name span, the web's inline code), so the
//! sentence is built once as segments and each surface decides how a name
//! looks. Nothing ever finds a name by parsing a rendered sentence.
//!
//! [`Prose::plain`] is the plain-text spelling, for logs and tests. A name
//! built with [`Prose::quoted`] is wrapped in straight double quotes there,
//! because that is how the sentence read before names were chips; the chip
//! replaces the quotes on both surfaces, and a padded terminal chip is exactly
//! as wide as the quoted text was.
//!
//! The browser's twin is `crates/dux-web/web/src/lib/prose.tsx`. Sentences both
//! surfaces print are pinned segment for segment by
//! `tests/fixtures/prose_cross_language.json`, which each side reads.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One piece of a sentence: constant words, or a name.
///
/// Serialized untagged, which IS the browser's `Prose` shape: a JSON string for
/// words, `{ "name", "quoted" }` for a name. The wire (a status's parts) and the
/// cross-language fixture both go through this one derive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProseSegment {
    Text(String),
    Name {
        name: String,
        /// The plain-text spelling wraps this name in straight double quotes.
        quoted: bool,
    },
}

/// A sentence as segments. Adjacent text is merged as it is pushed, so two
/// builders that say the same words produce equal values however they split
/// the constant parts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Prose {
    segments: Vec<ProseSegment>,
}

impl Prose {
    pub fn new() -> Self {
        Self::default()
    }

    /// Constant words.
    pub fn text(mut self, text: impl AsRef<str>) -> Self {
        self.push_text(text);
        self
    }

    /// A name the plain-text spelling leaves bare.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.push_name(name);
        self
    }

    /// A name the plain-text spelling wraps in straight double quotes.
    pub fn quoted(mut self, name: impl Into<String>) -> Self {
        self.push_quoted(name);
        self
    }

    /// Another sentence, appended segment by segment.
    pub fn then(mut self, other: Prose) -> Self {
        for segment in other.segments {
            match segment {
                ProseSegment::Text(text) => self.push_text(text),
                name @ ProseSegment::Name { .. } => self.segments.push(name),
            }
        }
        self
    }

    pub fn push_text(&mut self, text: impl AsRef<str>) {
        let text = text.as_ref();
        if text.is_empty() {
            return;
        }
        if let Some(ProseSegment::Text(last)) = self.segments.last_mut() {
            last.push_str(text);
        } else {
            self.segments.push(ProseSegment::Text(text.to_string()));
        }
    }

    /// A name is where text dux did not write (a branch, a pull request's head,
    /// a folder) enters a sentence, so this is where its bidi controls go: a
    /// chip is drawn as one unit inside dux's own words, and an override inside
    /// it would reorder them. Every name reaches a status through here too, so
    /// the plain message and the parts agree.
    pub fn push_name(&mut self, name: impl Into<String>) {
        self.segments.push(ProseSegment::Name {
            name: crate::bidi::strip_bidi_controls(&name.into()),
            quoted: false,
        });
    }

    /// See [`Prose::push_name`].
    pub fn push_quoted(&mut self, name: impl Into<String>) {
        self.segments.push(ProseSegment::Name {
            name: crate::bidi::strip_bidi_controls(&name.into()),
            quoted: true,
        });
    }

    pub fn segments(&self) -> &[ProseSegment] {
        &self.segments
    }

    /// Rebuild a sentence from segments (read off the wire, say), merging
    /// adjacent text the way the builders do.
    pub fn from_segments(segments: impl IntoIterator<Item = ProseSegment>) -> Self {
        let mut prose = Prose::new();
        for segment in segments {
            match segment {
                ProseSegment::Text(text) => prose.push_text(text),
                name @ ProseSegment::Name { .. } => prose.segments.push(name),
            }
        }
        prose
    }

    pub fn into_segments(self) -> Vec<ProseSegment> {
        self.segments
    }

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// The plain-text spelling: quoted names in straight double quotes, bare
    /// names as they are.
    pub fn plain(&self) -> String {
        let mut out = String::new();
        for segment in &self.segments {
            match segment {
                ProseSegment::Text(text) => out.push_str(text),
                ProseSegment::Name { name, quoted: true } => {
                    out.push('"');
                    out.push_str(name);
                    out.push('"');
                }
                ProseSegment::Name {
                    name,
                    quoted: false,
                } => out.push_str(name),
            }
        }
        out
    }

    /// The shape the cross-language fixture records: a string for constant
    /// words, `{ "name", "quoted" }` for a name, the same shape the browser's
    /// `Prose` type has.
    pub fn to_json(&self) -> Value {
        serde_json::to_value(&self.segments).expect("prose segments always serialize")
    }
}

impl From<&str> for Prose {
    fn from(text: &str) -> Self {
        Prose::new().text(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A chip draws a name as one unit inside dux's own sentence, so a name
    /// carrying an override would reorder the words around it. Every name
    /// enters a sentence through the name builders, which is where they go.
    #[test]
    fn a_name_arrives_without_bidi_controls() {
        let prose = Prose::new()
            .text("On ")
            .quoted("feat\u{202E}txt.exe\u{202C}")
            .text(" in ")
            .name("/src/\u{2066}x\u{2069}");
        assert_eq!(
            prose.segments(),
            &[
                ProseSegment::Text("On ".into()),
                ProseSegment::Name {
                    name: "feattxt.exe".into(),
                    quoted: true
                },
                ProseSegment::Text(" in ".into()),
                ProseSegment::Name {
                    name: "/src/x".into(),
                    quoted: false
                },
            ]
        );
        assert_eq!(prose.plain(), "On \"feattxt.exe\" in /src/x");
    }

    #[test]
    fn segments_read_back_from_their_json_shape() {
        let prose = Prose::new()
            .text("On ")
            .quoted("main")
            .text(" in ")
            .name("/src");
        let back: Vec<ProseSegment> = serde_json::from_value(prose.to_json()).unwrap();
        assert_eq!(Prose::from_segments(back), prose);
    }

    #[test]
    fn the_plain_spelling_quotes_only_the_names_built_quoted() {
        let prose = Prose::new()
            .text("Delete ")
            .quoted("feat/login")
            .text(" at ")
            .name("~/src/dux")
            .text("?");
        assert_eq!(prose.plain(), "Delete \"feat/login\" at ~/src/dux?");
    }

    #[test]
    fn adjacent_text_merges_so_equal_words_are_equal_values() {
        let split = Prose::new().text("one ").text("two ").name("x");
        let whole = Prose::new().text("one two ").name("x");
        assert_eq!(split, whole);
        assert_eq!(split.segments().len(), 2);
    }

    #[test]
    fn then_appends_and_merges_across_the_seam() {
        let prose = Prose::new()
            .name("a")
            .text(" and")
            .then(Prose::new().text(" more ").quoted("b"));
        assert_eq!(
            prose.segments(),
            &[
                ProseSegment::Name {
                    name: "a".into(),
                    quoted: false
                },
                ProseSegment::Text(" and more ".into()),
                ProseSegment::Name {
                    name: "b".into(),
                    quoted: true
                },
            ]
        );
    }

    #[test]
    fn empty_text_adds_no_segment() {
        assert!(Prose::new().text("").is_empty());
    }

    #[test]
    fn the_json_shape_matches_the_browsers_prose_type() {
        let prose = Prose::new().text("On ").quoted("main").text(".");
        assert_eq!(
            prose.to_json(),
            json!(["On ", { "name": "main", "quoted": true }, "."])
        );
    }

    /// Merge adjacent strings, the normalization the fixture's note promises,
    /// so neither side is bound to the other's split of its constant words.
    fn merged(segments: &Value) -> Value {
        let mut out: Vec<Value> = Vec::new();
        for segment in segments.as_array().expect("segments are an array") {
            match (out.last_mut(), segment) {
                (Some(Value::String(last)), Value::String(next)) => last.push_str(next),
                _ => out.push(segment.clone()),
            }
        }
        Value::Array(out)
    }

    /// Build one fixture case's sentence the way the terminal UI does.
    fn build(sentence: &str, args: &Value) -> Prose {
        let s = |key: &str| args[key].as_str().expect(key).to_string();
        match sentence {
            "detach_confirm" => {
                let label = s("label");
                let grace = args["grace_seconds"].as_u64().expect("grace_seconds");
                let tabs = args["live_tabs"].as_u64().expect("live_tabs") as usize;
                crate::engine::detach_confirm_prose(&label, grace, tabs)
            }
            "recreate_confirm" => {
                let worktree = std::path::PathBuf::from(s("worktree_label"));
                let providers: Vec<String> = args["providers"]
                    .as_array()
                    .expect("providers")
                    .iter()
                    .map(|p| p.as_str().expect("provider").to_string())
                    .collect();
                let resumes = args["conversation_resumes"].as_bool().expect("resumes");
                let (branch, source) = (s("branch_name"), s("source_branch"));
                crate::working_copy::recreate_confirm_prose(
                    &worktree, &branch, &source, resumes, &providers,
                )
            }
            "checkout_default_branch_confirm" => {
                let project = s("project_name");
                let base = args["stored_base"].as_str();
                crate::engine::checkout_default_branch_confirm_prose(&project, base)
            }
            "change_base_branch_confirm" => crate::engine::change_base_branch_confirm_prose(
                &s("project_name"),
                args["from"].as_str(),
                &s("to"),
            ),
            "add_project_branch_warning" => crate::add_project_prose::branch_warning_prose(
                &s("current_branch"),
                args["default_branch"].as_str(),
            ),
            "add_project_worktree_base" => {
                crate::add_project_prose::worktree_base_note_prose(&s("branch"))
            }
            "add_project_heuristic_note" => crate::add_project_prose::heuristic_branch_note_prose(),
            "delete_project_confirm" => crate::project_prose::delete_project_confirm_prose(
                args["project_name"].as_str(),
                args["agent_count"].as_u64().expect("agent_count") as usize,
            ),
            "remove_project_confirm" => crate::project_prose::remove_project_confirm_prose(
                args["project_name"].as_str(),
                args["agent_count"].as_u64().expect("agent_count") as usize,
            ),
            other => panic!("the fixture names a sentence this test cannot build: {other}"),
        }
    }

    /// The other half of the pin in the browser's `prose.test.tsx`. Both read
    /// `tests/fixtures/prose_cross_language.json`, so a sentence passes only
    /// when the terminal UI and the web say the same words and mark the same
    /// names.
    #[test]
    fn the_two_surfaces_build_every_shared_sentence_from_the_same_segments() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/prose_cross_language.json");
        let raw = std::fs::read_to_string(&path).expect("fixture must be readable");
        let fixture: Value = serde_json::from_str(&raw).expect("fixture must parse");
        let cases = fixture["cases"].as_array().expect("cases");
        assert!(cases.len() >= 10, "the fixture has lost its cases");
        for case in cases {
            let what = case["what"].as_str().expect("what");
            let sentence = case["sentence"].as_str().expect("sentence");
            let prose = build(sentence, &case["args"]);
            assert_eq!(
                merged(&prose.to_json()),
                merged(&case["segments"]),
                "{what}: the terminal UI's segments differ from the fixture"
            );
        }
    }
}
