//! A status sentence as the status line and the web's toasts need it: the plain
//! message every surface has always read, and, when the producer built it from
//! parts, the [`Prose`] behind it so the web can draw each name as a chip.
//!
//! The parts ARE [`crate::prose`]: one segment type and one JSON shape for the
//! dialogs and the statuses alike. This module only adds what a status needs on
//! top: a sentence handed over as a finished string has no parts at all (the
//! wire leaves the field out and the browser shows the text as it always did),
//! and the plain message is carried beside the parts rather than recomputed, so
//! every existing reader of `message` keeps its exact bytes. The terminal UI's
//! status line reads the message and nothing else.
//!
//! Build one with [`status_text!`](crate::status_text!): `q(name)` for a name
//! the plain spelling wraps in straight double quotes, `n(name)` for a bare one,
//! anything else for words or a whole sentence spliced in.

use std::borrow::Cow;
use std::fmt;

use crate::prose::{Prose, ProseSegment};

/// A status sentence: its plain spelling, and the prose it was built from when
/// it was built from parts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusText {
    message: String,
    /// `None` for a sentence that arrived as a finished string. When `Some`,
    /// `prose.plain() == message`, by construction.
    prose: Option<Prose>,
    /// The question this sentence puts to whoever reads it, when it is one.
    ask: Option<StatusAsk>,
}

/// A question a refusal puts to the surface that shows it, so that surface can
/// offer the answer where the user is instead of leaving a sentence to retype
/// the request from. The sentence still says everything in words; this is the
/// part a surface cannot get by reading them.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StatusAsk {
    /// Creating an agent from a pull request needs a review copy the user has
    /// not agreed to. `warned` identifies what the warning said (why the
    /// branch could not be used, and the copy's name), and goes back with the
    /// agreement so the server can tell the user agreed to this copy and not
    /// to a different one made necessary since. `warning` is the warning alone,
    /// in parts: the sentence of the status adds how to agree on the command
    /// line, which a dialog with a checkbox under it has no use for.
    ///
    /// Boxed so the question costs a pointer on every status that never asks.
    ReviewCopy(Box<ReviewCopyAsk>),
}

/// What a refusal for want of agreement to a review copy asks; see
/// [`StatusAsk::ReviewCopy`].
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReviewCopyAsk {
    pub warned: String,
    pub warning: Vec<ProseSegment>,
    /// The id the asking client gave the create, so it can tell this is the
    /// answer to that create and not to a later one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    /// The request came from the command line, which agrees with a flag.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub by_flag: bool,
    /// Set when a clone or an add made the project and stopped short of the
    /// agent for want of this answer: the project the agent goes in, so the
    /// surface can reopen New agent from PR for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// The pull request that create was for, set with `project_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request: Option<crate::pr_reference::PullRequestCloneTarget>,
}

/// Whether creating an agent from a pull request will need a review copy,
/// told before the project exists or the folder is added, so a surface can put
/// the question beside the button that would start it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ReviewCopyOutlook {
    /// The branch can be used as it is.
    NotNeeded,
    /// Nothing available before the clone or the add can say. The question, if
    /// there is one, is asked after the project exists.
    Unknown,
    /// A copy will be needed.
    Needed {
        /// Why, as the word a [`ReviewCopyAsk::warned`] begins with: `busy`,
        /// `diverged`, `unknown` or `holder-unknown`. Agreeing with this word
        /// covers a copy for the same reason under whatever name turns out
        /// free.
        reason: String,
        /// The pull request's own branch, which is left alone.
        branch: String,
        /// The branch the copy would be made on.
        copy_branch: String,
        /// The warning, in parts.
        warning: Vec<ProseSegment>,
    },
    /// A copy will be needed and every name it could take is taken, so nothing
    /// can be done until the user types another name for the agent.
    NoFreeName {
        reason: String,
        branch: String,
        /// The refusal's sentence, in parts.
        message: Vec<ProseSegment>,
    },
}

impl StatusText {
    /// An empty structured sentence.
    pub fn new() -> Self {
        Self::from(Prose::new())
    }

    /// A finished string with no structure: what every unconverted site passes.
    pub fn plain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            prose: None,
            ask: None,
        }
    }

    /// Reassemble a sentence from the two halves a status carries. The parts are
    /// kept only when they spell the message, so a copy can never put different
    /// words beside the ones the terminal UI prints.
    pub fn from_parts(message: String, segments: Option<Vec<ProseSegment>>) -> Self {
        let prose = segments.map(Prose::from_segments).filter(|prose| {
            let spelled = prose.plain();
            let agrees = spelled == message;
            if !agrees {
                // The fallback hides the disagreement from the user, so say it
                // where a developer can find it: a producer upstream is wrong.
                crate::logger::debug(&format!(
                    "status parts do not spell their message, keeping the plain text: \
                     message {message:?}, parts spell {spelled:?}"
                ));
            }
            agrees
        });
        Self {
            message,
            prose,
            ask: None,
        }
    }

    /// This sentence, marked as a question to put to the user.
    #[must_use]
    pub fn with_ask(mut self, ask: StatusAsk) -> Self {
        self.ask = Some(ask);
        self
    }

    /// The question this sentence puts, if it is one.
    pub fn ask(&self) -> Option<&StatusAsk> {
        self.ask.as_ref()
    }

    /// The plain spelling: what the terminal UI prints, word for word.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The prose, or `None` when the sentence carries no structure.
    pub fn prose(&self) -> Option<&Prose> {
        self.prose.as_ref()
    }

    /// The parts, or `None` when the sentence carries no structure.
    pub fn segments(&self) -> Option<&[ProseSegment]> {
        self.prose.as_ref().map(Prose::segments)
    }

    /// Split into the plain spelling and the parts.
    pub fn into_parts(self) -> (String, Option<Vec<ProseSegment>>) {
        (self.message, self.prose.map(Prose::into_segments))
    }

    pub fn is_empty(&self) -> bool {
        self.message.is_empty()
    }

    /// Append another sentence: words, or a whole sentence built elsewhere,
    /// whose names come along.
    pub fn push(&mut self, part: impl Into<StatusText>) {
        let part = part.into();
        if part.message.is_empty() {
            return;
        }
        let own = self
            .prose
            .take()
            .unwrap_or_else(|| Prose::new().text(&self.message));
        let joined = match part.prose {
            Some(prose) => own.then(prose),
            None => own.text(&part.message),
        };
        self.message.push_str(&part.message);
        self.prose = Some(joined);
    }

    /// Append a name the plain spelling wraps in straight double quotes.
    pub fn push_quoted(&mut self, name: impl fmt::Display) {
        self.push(Prose::new().quoted(name.to_string()));
    }

    /// Append a name the plain spelling leaves bare (a path, a URL, a number).
    pub fn push_name(&mut self, name: impl fmt::Display) {
        self.push(Prose::new().name(name.to_string()));
    }
}

/// Build a [`StatusText`] from its parts, in reading order.
///
/// ```
/// use dux_core::status_text;
/// let text = status_text!["Checked out ", q("main"), " in ", n("/src/app"), "."];
/// assert_eq!(text, "Checked out \"main\" in /src/app.");
/// ```
///
/// - `q(expr)`: a name the plain spelling wraps in straight double quotes;
/// - `n(expr)`: a name the plain spelling leaves bare;
/// - anything else: words, or a whole [`StatusText`] or [`Prose`] spliced in.
#[macro_export]
macro_rules! status_text {
    ($($part:tt)*) => {{
        #[allow(unused_mut)]
        let mut __status_text = $crate::status_text::StatusText::new();
        $crate::__status_text_parts!(__status_text; $($part)*);
        __status_text
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __status_text_parts {
    ($t:ident;) => {};
    ($t:ident; q($e:expr) $(, $($rest:tt)*)?) => {
        $t.push_quoted(&$e);
        $crate::__status_text_parts!($t; $($($rest)*)?);
    };
    ($t:ident; n($e:expr) $(, $($rest:tt)*)?) => {
        $t.push_name(&$e);
        $crate::__status_text_parts!($t; $($($rest)*)?);
    };
    ($t:ident; $e:expr $(, $($rest:tt)*)?) => {
        $t.push($e);
        $crate::__status_text_parts!($t; $($($rest)*)?);
    };
}

impl From<Prose> for StatusText {
    fn from(prose: Prose) -> Self {
        Self {
            message: prose.plain(),
            prose: Some(prose),
            ask: None,
        }
    }
}

impl From<String> for StatusText {
    fn from(message: String) -> Self {
        Self::plain(message)
    }
}

impl From<&str> for StatusText {
    fn from(message: &str) -> Self {
        Self::plain(message)
    }
}

impl From<&String> for StatusText {
    fn from(message: &String) -> Self {
        Self::plain(message.as_str())
    }
}

impl From<Cow<'_, str>> for StatusText {
    fn from(message: Cow<'_, str>) -> Self {
        Self::plain(message.into_owned())
    }
}

impl From<&StatusText> for StatusText {
    fn from(text: &StatusText) -> Self {
        text.clone()
    }
}

impl From<StatusText> for String {
    fn from(text: StatusText) -> Self {
        text.message
    }
}

/// A sentence reads as its plain spelling wherever it is only looked at
/// (`contains`, `starts_with`, a log line), so a producer that builds one from
/// parts changes nothing for a reader that only ever wanted the words.
impl std::ops::Deref for StatusText {
    type Target = str;

    fn deref(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for StatusText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl PartialEq<str> for StatusText {
    fn eq(&self, other: &str) -> bool {
        self.message == other
    }
}

impl PartialEq<&str> for StatusText {
    fn eq(&self, other: &&str) -> bool {
        self.message == *other
    }
}

impl PartialEq<String> for StatusText {
    fn eq(&self, other: &String) -> bool {
        &self.message == other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The plain message the terminal UI prints and the parts the web chips
    /// come from the same names, so both lose the bidi controls together.
    #[test]
    fn a_status_name_is_stripped_in_the_message_and_the_parts_alike() {
        let status = crate::status_text!["Deleted ", q("feat\u{202E}xe"), " in ", n("/r\u{200F}")];
        assert_eq!(status.message(), "Deleted \"featxe\" in /r");
        assert_eq!(
            status.segments().expect("built from parts"),
            &[
                text("Deleted "),
                name("featxe", true),
                text(" in "),
                name("/r", false)
            ]
        );
    }

    fn text(s: &str) -> ProseSegment {
        ProseSegment::Text(s.to_string())
    }

    fn name(s: &str, quoted: bool) -> ProseSegment {
        ProseSegment::Name {
            name: s.to_string(),
            quoted,
        }
    }

    #[test]
    fn a_quoted_name_keeps_its_quotes_in_the_plain_spelling_and_drops_them_in_the_segment() {
        let built = crate::status_text![
            "Checked out ",
            q("main"),
            " for project ",
            q("p1-name"),
            "."
        ];
        assert_eq!(
            built.message(),
            "Checked out \"main\" for project \"p1-name\"."
        );
        assert_eq!(
            built.segments().unwrap(),
            &[
                text("Checked out "),
                name("main", true),
                text(" for project "),
                name("p1-name", true),
                text("."),
            ]
        );
    }

    #[test]
    fn a_bare_name_stays_bare_in_the_plain_spelling() {
        let built = crate::status_text!["Saved to ", n("/tmp/a b/c.txt"), "."];
        assert_eq!(built.message(), "Saved to /tmp/a b/c.txt.");
        assert_eq!(
            built.segments().unwrap(),
            &[text("Saved to "), name("/tmp/a b/c.txt", false), text(".")]
        );
    }

    #[test]
    fn a_leading_name_has_no_empty_text_before_it() {
        let built = crate::status_text![q("feature"), " is gone."];
        assert_eq!(
            built.segments().unwrap(),
            &[name("feature", true), text(" is gone.")]
        );
    }

    #[test]
    fn a_plain_string_carries_no_segments() {
        let plain: StatusText = format!("Nothing to name {}.", 3).into();
        assert_eq!(plain.message(), "Nothing to name 3.");
        assert_eq!(plain.segments(), None);
    }

    #[test]
    fn splicing_a_structured_sentence_keeps_its_names_and_merges_adjacent_words() {
        let suffix = crate::status_text![" New worktrees branch from ", q("main"), " now."];
        let built = crate::status_text!["Checked out ", q("main"), ".", suffix];
        assert_eq!(
            built.message(),
            "Checked out \"main\". New worktrees branch from \"main\" now."
        );
        assert_eq!(
            built.segments().unwrap(),
            &[
                text("Checked out "),
                name("main", true),
                text(". New worktrees branch from "),
                name("main", true),
                text(" now."),
            ]
        );
    }

    #[test]
    fn splicing_an_empty_sentence_changes_nothing() {
        let built = crate::status_text!["Done", String::new(), "."];
        assert_eq!(built.segments().unwrap(), &[text("Done.")]);
    }

    #[test]
    fn a_plain_sentence_gains_structure_when_a_name_is_pushed_onto_it() {
        let mut built = StatusText::plain("Removed ");
        built.push_quoted("dev");
        assert_eq!(built.message(), "Removed \"dev\"");
        assert_eq!(
            built.segments().unwrap(),
            &[text("Removed "), name("dev", true)]
        );
    }

    #[test]
    fn a_dialog_prose_spliced_in_keeps_its_names() {
        let prose = Prose::new().text("agent ").quoted("feat");
        let built = crate::status_text!["Stopped ", prose, "."];
        assert_eq!(built.message(), "Stopped agent \"feat\".");
        assert_eq!(
            built.segments().unwrap(),
            &[text("Stopped agent "), name("feat", true), text(".")]
        );
    }

    #[test]
    fn names_with_multibyte_characters_are_carried_whole() {
        let built = crate::status_text!["Añadido ", q("rama-ñ✓"), " ✓"];
        assert_eq!(built.message(), "Añadido \"rama-ñ✓\" ✓");
        assert_eq!(
            built.segments().unwrap(),
            &[text("Añadido "), name("rama-ñ✓", true), text(" ✓")]
        );
    }

    #[test]
    fn segments_serialize_as_the_browsers_prose_shape() {
        let built = crate::status_text!["Pushed ", q("main"), " to ", n("origin"), "."];
        let json = serde_json::to_value(built.segments().unwrap()).unwrap();
        assert_eq!(
            json,
            serde_json::json!([
                "Pushed ",
                {"name": "main", "quoted": true},
                " to ",
                {"name": "origin", "quoted": false},
                "."
            ])
        );
        assert_eq!(json, built.prose().unwrap().to_json(), "one JSON shape");
    }

    #[test]
    fn parts_that_do_not_spell_the_message_are_dropped_on_reassembly() {
        let built = crate::status_text!["On ", q("main"), "."];
        let (message, segments) = built.clone().into_parts();
        assert_eq!(StatusText::from_parts(message, segments.clone()), built);
        let (other, logged) = crate::logger::capture_for_test(|| {
            StatusText::from_parts("Something else.".into(), segments)
        });
        assert_eq!(other.segments(), None);
        assert_eq!(other.message(), "Something else.");
        // A disagreement means a producer upstream is wrong; the fallback hides
        // it from the user, so the log is where it can still be found.
        assert!(
            logged
                .iter()
                .any(|line| line.starts_with("DEBUG") && line.contains("Something else.")),
            "{logged:?}"
        );
    }

    #[test]
    fn it_compares_equal_to_its_plain_spelling() {
        let built = crate::status_text!["Opened ", n(42), "."];
        assert_eq!(built, "Opened 42.");
        assert_eq!(built, "Opened 42.".to_string());
        assert_eq!(built.to_string(), "Opened 42.");
        assert_eq!(String::from(built), "Opened 42.");
    }
}
