//! Why a tab's last run ended, kept so the dormant card can say it rather than
//! only "something went wrong". The case this exists for is a provider that
//! printed the answer on its way out, such as `codex resume --last` exiting 1 in
//! a fifth of a second because the same conversation is open in a companion
//! terminal.
//!
//! The engine records a verdict: how the run ended, when, and the last lines it
//! had on screen. Memory-only, never persisted, cleared when a launch is
//! dispatched for the tab, and forgotten with the row. A deliberate teardown
//! clears it too, through `Engine::clear_tab_runtime`, so the prune records its
//! verdict after that teardown.
//!
//! The prose lives here rather than in either surface because both cards say it
//! in the same words; the browser ports [`humanize_age_ago`] and
//! [`ending_sentence`] and pins the port in its own tests.

use std::time::Instant;

/// How many lines of the run's final output the verdict keeps. Small on
/// purpose: this is the tail a person reads at a glance on a card, not a log.
/// A provider that dies with a stack trace is not diagnosed here; it is
/// diagnosed in `dux.log`.
pub const VERDICT_EXCERPT_MAX_LINES: usize = 8;

/// How many CHARACTERS (never bytes) of each excerpt line are kept. Wide enough
/// for an ordinary error sentence, bounded so one pathological line cannot ride
/// the wire to every connected browser.
pub const VERDICT_EXCERPT_MAX_CHARS: usize = 200;

/// The exhaustive set of ways a tab's run can have ended badly.
///
/// Exhaustive rather than a string so a new ending is a compile error in every
/// place that words one, and so the wire kind has exactly one spelling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TabRunEnding {
    /// The provider never started: the spawn itself failed, and this is what it
    /// said. Nothing ran, so there is no status and usually no output.
    LaunchFailed { error: String },
    /// The child was reaped and its status was non-zero.
    ExitedWithStatus { status: u32 },
    /// The child's PTY ended without dux ever seeing a status (a read error, or
    /// a child that closed its descriptors and was never reapable). "Exited,
    /// status unknown" is a genuinely different answer from any number.
    ExitedUnknownStatus,
    /// The child exited CLEANLY but was over inside
    /// [`crate::engine::RAPID_EXIT_WINDOW`] with nobody typing into
    /// it, which dux reads as a provider that never came up.
    RapidCleanExit,
}

impl TabRunEnding {
    /// The stable wire spelling. Kept next to the variants so the two cannot
    /// drift, and matched exhaustively so a new variant must choose one.
    pub fn wire_kind(&self) -> &'static str {
        match self {
            TabRunEnding::LaunchFailed { .. } => "launch_failed",
            TabRunEnding::ExitedWithStatus { .. } => "exited",
            TabRunEnding::ExitedUnknownStatus => "exited_unknown",
            TabRunEnding::RapidCleanExit => "rapid_clean_exit",
        }
    }

    /// The exit status, for the one ending that has one.
    pub fn status(&self) -> Option<u32> {
        match self {
            TabRunEnding::ExitedWithStatus { status } => Some(*status),
            TabRunEnding::LaunchFailed { .. }
            | TabRunEnding::ExitedUnknownStatus
            | TabRunEnding::RapidCleanExit => None,
        }
    }

    /// The spawn error, for the one ending that has one.
    pub fn error(&self) -> Option<&str> {
        match self {
            TabRunEnding::LaunchFailed { error } => Some(error.as_str()),
            TabRunEnding::ExitedWithStatus { .. }
            | TabRunEnding::ExitedUnknownStatus
            | TabRunEnding::RapidCleanExit => None,
        }
    }
}

/// What a tab's LAST run did, recorded at the moment the run ended.
///
/// `ended_at` is an `Instant` rather than a wall clock for the same reason every
/// other age in dux is: the card wants "how long ago", and a `SystemTime` can
/// step backwards under an NTP correction and print a negative age.
#[derive(Clone, Debug)]
pub struct TabRunVerdict {
    pub ending: TabRunEnding,
    pub ended_at: Instant,
    /// The last visible non-blank lines the run had on screen, captured ONCE at
    /// the moment the run ended (the client is gone immediately afterwards) and
    /// never topped up later.
    pub excerpt: Vec<String>,
}

impl TabRunVerdict {
    pub fn new(ending: TabRunEnding, excerpt: Vec<String>) -> Self {
        Self {
            ending,
            ended_at: Instant::now(),
            excerpt,
        }
    }

    pub fn ended_seconds_ago(&self) -> u64 {
        self.ended_at.elapsed().as_secs()
    }
}

/// The last [`VERDICT_EXCERPT_MAX_LINES`] non-blank lines of a run's visible
/// output, each cut to [`VERDICT_EXCERPT_MAX_CHARS`] characters.
///
/// The tail rather than the head: a CLI that fails says so on its way out, and
/// the head of a screenful is a banner. Char-based throughout, because provider
/// output is arbitrary UTF-8 and byte slicing panics inside a character.
pub fn verdict_excerpt(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect();
    let start = lines.len().saturating_sub(VERDICT_EXCERPT_MAX_LINES);
    lines[start..]
        .iter()
        .map(|line| crate::device_label::truncate_chars(line, VERDICT_EXCERPT_MAX_CHARS))
        .collect()
}

/// How many of the run's LAST excerpt lines a refused resume quotes. Two, not
/// the whole excerpt: this sentence lands on a status line and in a toast, and
/// a CLI's refusal says the thing and then says what to do about it.
pub const RESUME_REFUSAL_QUOTED_LINES: usize = 2;

/// The warning both surfaces raise when a RESUME run was refused: the provider
/// came up, said why it would not resume, and quit at once.
///
/// The provider's own words carry the remedy, so dux quotes them rather than
/// interpreting them. Nothing here knows any provider: a CLI that refuses for a
/// reason dux has never heard of is reported exactly as well as one that does
/// not. `None` when there is nothing to quote, which leaves the caller with the
/// ordinary exit wording rather than an empty pair of words.
///
/// `remedy` is the surface's own closing sentence, passed in rather than written
/// here: what to do next is different on a page you can click and in a terminal
/// with a keybinding, and a shared sentence would have to be vague enough to fit
/// both. What must not differ is everything before it, which is why the quote
/// and its cut marks live here.
pub fn refused_resume_warning(
    agent_label: &str,
    excerpt: &[String],
    remedy: &str,
) -> Option<String> {
    let start = excerpt.len().saturating_sub(RESUME_REFUSAL_QUOTED_LINES);
    let quote = excerpt[start..]
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if quote.is_empty() {
        return None;
    }
    // The excerpt caps each LINE; the joined quote is capped again so two full
    // lines cannot double the budget on their way to a toast.
    let quote = crate::device_label::truncate_chars(&quote, VERDICT_EXCERPT_MAX_CHARS);
    // Both ends of the quote say whether they are a cut. There were rows before
    // these ones whenever the excerpt held more than the quoted lines, and the
    // reader has no other way to know the provider said more.
    let head = if excerpt.len() > RESUME_REFUSAL_QUOTED_LINES {
        "\u{2026} "
    } else {
        ""
    };
    // A quote that ended its own sentence is left exactly as the provider wrote
    // it; adding a second full stop reads as a typo rather than as a quotation.
    // Anything else was cut, by a wrapped terminal row or by the character cap,
    // and an ellipsis is the honest mark for that. A full stop there would be
    // dux claiming the provider finished a sentence it did not finish.
    let tail = if quote.ends_with(['.', '!', '?', '\u{2026}']) {
        ""
    } else {
        "\u{2026}"
    };
    Some(format!(
        "Agent \"{agent_label}\" could not resume its previous session; the provider said: \
         {head}{quote}{tail} {remedy}"
    ))
}

/// "moments ago", "about 2 minutes ago": a coarse, prose-shaped age for a
/// sentence, deliberately not the compact `2m` the agent list uses.
///
/// Coarse on purpose. The card answers "was this just now, or this morning";
/// a card that claimed "1 minute 43 seconds ago" would be precise about a
/// number nobody acts on.
pub fn humanize_age_ago(seconds: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    match seconds {
        s if s < 45 => "moments ago".to_string(),
        s if s < 90 => "about a minute ago".to_string(),
        // Every boundary is the ROUNDED value's, not the raw one's, or the
        // rounding walks a value into the next unit's vocabulary: 59.5 minutes
        // would otherwise print "about 60 minutes ago".
        s if s < HOUR - MINUTE / 2 => format!("about {} minutes ago", (s + MINUTE / 2) / MINUTE),
        s if s < 90 * MINUTE => "about an hour ago".to_string(),
        s if s < DAY - HOUR / 2 => format!("about {} hours ago", (s + HOUR / 2) / HOUR),
        s if s < 36 * HOUR => "about a day ago".to_string(),
        s => format!("about {} days ago", (s + DAY / 2) / DAY),
    }
}

/// A duration in words, for a sentence: "five seconds", "12 seconds".
///
/// The rapid-exit window is a constant, and a sentence that spells it out by
/// hand goes quietly wrong the day somebody tunes it. Small numbers are spelled
/// because that is how the rest of dux's prose reads; past ten a digit is
/// clearer.
fn spell_seconds(seconds: u64) -> String {
    const WORDS: [&str; 11] = [
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
    ];
    let unit = if seconds == 1 { "second" } else { "seconds" };
    match WORDS.get(seconds as usize) {
        Some(word) => format!("{word} {unit}"),
        None => format!("{seconds} {unit}"),
    }
}

/// The one sentence both dormant cards print about how the last run ended.
///
/// Deliberately neutral about blame: a non-zero exit is often the user quitting
/// a CLI in a way it reports as an error, so the sentence says what dux observed
/// and what dux therefore did NOT do, never that anything crashed.
pub fn ending_sentence(ending: &TabRunEnding, seconds_ago: u64) -> String {
    let age = humanize_age_ago(seconds_ago);
    match ending {
        TabRunEnding::LaunchFailed { error } => {
            format!("Its last run could not be launched {age}: {error}")
        }
        TabRunEnding::ExitedWithStatus { status } => format!(
            "Its last run exited with status {status} {age}, so dux didn't start it again on its own."
        ),
        TabRunEnding::ExitedUnknownStatus => format!(
            "Its last run exited with an unknown status {age}, so dux didn't start it again on its own."
        ),
        TabRunEnding::RapidCleanExit => {
            let window = spell_seconds(crate::engine::RAPID_EXIT_WINDOW.as_secs());
            format!(
                "Its last run ended {age} in under {window} with status 0, which dux treats as a run that never came up."
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for a surface's own closing sentence.
    const REMEDY: &str = "Open the agent to see the full output, or start a fresh session.";

    #[test]
    fn the_excerpt_keeps_the_tail_not_the_head() {
        let text = (1..=20)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let excerpt = verdict_excerpt(&text);
        assert_eq!(excerpt.len(), VERDICT_EXCERPT_MAX_LINES);
        assert_eq!(
            excerpt.first().map(String::as_str),
            Some("line 13"),
            "a failing CLI says why on its way OUT, so the tail is the part worth keeping"
        );
        assert_eq!(excerpt.last().map(String::as_str), Some("line 20"));
    }

    #[test]
    fn the_excerpt_drops_blank_lines_and_cuts_long_ones_by_character() {
        let long: String = "\u{4e2d}".repeat(400);
        let excerpt = verdict_excerpt(&format!("first\n\n   \n{long}"));
        assert_eq!(excerpt.len(), 2, "blank and whitespace-only lines are gone");
        assert_eq!(excerpt[0], "first");
        assert_eq!(
            excerpt[1].chars().count(),
            VERDICT_EXCERPT_MAX_CHARS,
            "the cut counts characters; a byte cut would have panicked inside one"
        );
        assert!(excerpt[1].ends_with('\u{2026}'));
    }

    #[test]
    fn ages_read_as_prose() {
        assert_eq!(humanize_age_ago(0), "moments ago");
        assert_eq!(humanize_age_ago(44), "moments ago");
        assert_eq!(humanize_age_ago(45), "about a minute ago");
        assert_eq!(humanize_age_ago(120), "about 2 minutes ago");
        assert_eq!(humanize_age_ago(3599), "about an hour ago");
        assert_eq!(humanize_age_ago(7200), "about 2 hours ago");
        assert_eq!(humanize_age_ago(86_400), "about a day ago");
        assert_eq!(humanize_age_ago(3 * 86_400), "about 3 days ago");
    }

    #[test]
    fn every_ending_words_itself_with_its_age() {
        assert_eq!(
            ending_sentence(&TabRunEnding::ExitedWithStatus { status: 1 }, 120),
            "Its last run exited with status 1 about 2 minutes ago, so dux didn't start it again on its own."
        );
        assert_eq!(
            ending_sentence(&TabRunEnding::ExitedUnknownStatus, 0),
            "Its last run exited with an unknown status moments ago, so dux didn't start it again on its own."
        );
        assert_eq!(
            ending_sentence(
                &TabRunEnding::LaunchFailed {
                    error: "no such file".to_string()
                },
                0
            ),
            "Its last run could not be launched moments ago: no such file"
        );
        assert_eq!(
            ending_sentence(&TabRunEnding::RapidCleanExit, 120),
            "Its last run ended about 2 minutes ago in under five seconds with status 0, which dux treats as a run that never came up."
        );
    }

    /// The window in the sentence IS the constant, so tuning
    /// `RAPID_EXIT_WINDOW` cannot leave the card telling users a stale number.
    #[test]
    fn the_rapid_window_sentence_is_built_from_the_constant() {
        let window = crate::engine::RAPID_EXIT_WINDOW.as_secs();
        assert_eq!(spell_seconds(window), "five seconds", "today's value");
        assert!(
            ending_sentence(&TabRunEnding::RapidCleanExit, 0)
                .contains(&format!("in under {}", spell_seconds(window))),
            "the sentence must quote the constant, not a hand-written number"
        );
        assert_eq!(spell_seconds(1), "one second");
        assert_eq!(spell_seconds(10), "ten seconds");
        assert_eq!(spell_seconds(11), "11 seconds");
    }

    #[test]
    fn a_refused_resume_quotes_the_provider_and_says_what_to_do() {
        let warning = refused_resume_warning(
            "feat/x",
            &[
                "Resuming your conversation.".to_string(),
                "Looking for a session to continue.".to_string(),
                "Found session 9f2 for this directory.".to_string(),
                "That session cannot be continued here.".to_string(),
                "Your most recent conversation is running in the background (session 9f2)."
                    .to_string(),
                "Use `claude agents` to find and attach to it.".to_string(),
            ],
            "Open the agent to see the full output, or start a fresh session.",
        )
        .expect("real provider output produces the refusal warning");
        assert_eq!(
            warning,
            "Agent \"feat/x\" could not resume its previous session; the provider said: \u{2026} \
             Your most recent conversation is running in the background (session 9f2). Use \
             `claude agents` to find and attach to it. Open the agent to see the full output, or \
             start a fresh session."
        );
    }

    /// Only the LAST couple of lines are quoted, so a screenful of banner does
    /// not ride into a status line, and the quote itself is capped by the same
    /// character budget the excerpt lines are. Both cuts are marked: the head
    /// one because the provider said more before these rows, the tail one
    /// because the budget stopped mid-sentence.
    #[test]
    fn the_refusal_quotes_the_tail_and_caps_it() {
        let excerpt: Vec<String> = (1..=6).map(|n| format!("line {n}")).collect();
        let warning = refused_resume_warning("feat/x", &excerpt, REMEDY).expect("a warning");
        assert!(
            warning.contains("said: \u{2026} line 5 line 6\u{2026}"),
            "only the last {RESUME_REFUSAL_QUOTED_LINES} lines are quoted, got {warning:?}"
        );
        assert!(!warning.contains("line 4"));

        let long = vec!["x".repeat(500)];
        let capped = refused_resume_warning("feat/x", &long, REMEDY).expect("a warning");
        assert!(
            capped.contains(&format!(
                "said: {}\u{2026} ",
                "x".repeat(VERDICT_EXCERPT_MAX_CHARS - 1)
            )),
            "one line is cut at the character budget, with no head mark, got {capped:?}"
        );
    }

    /// What the mark at the end of the quote means. A provider that finished its
    /// sentence is quoted verbatim; anything else was cut, by a wrapped terminal
    /// row or by the character cap, and an ellipsis is the honest mark for that.
    /// A full stop there would be dux finishing a sentence the provider did not.
    #[test]
    fn the_quote_ends_in_an_ellipsis_unless_the_provider_ended_its_own_sentence() {
        for finished in [
            "it is running.",
            "is it running?",
            "it is running!",
            "it is running\u{2026}",
        ] {
            let warning =
                refused_resume_warning("a", &[finished.to_string()], REMEDY).expect("a warning");
            assert!(
                warning.contains(&format!("said: {finished} Open")),
                "a finished sentence is quoted exactly as written, got {warning:?}"
            );
        }
        for cut in [
            "it is running,",
            "it is running;",
            "it is running-",
            "it is runnin",
            "attach to it:",
        ] {
            let warning =
                refused_resume_warning("a", &[cut.to_string()], REMEDY).expect("a warning");
            assert!(
                warning.contains(&format!("said: {cut}\u{2026} Open")),
                "a quote the row cut says it was cut, got {warning:?}"
            );
        }
    }

    /// Nothing to quote means nothing to say in the provider's words, so the
    /// caller keeps whatever wording it already had.
    #[test]
    fn a_refusal_with_nothing_to_quote_has_no_warning_of_its_own() {
        assert!(refused_resume_warning("feat/x", &[], REMEDY).is_none());
        assert!(refused_resume_warning("feat/x", &["   ".to_string()], REMEDY).is_none());
    }

    #[test]
    fn the_wire_kind_and_the_payload_agree_per_variant() {
        let launch = TabRunEnding::LaunchFailed {
            error: "boom".to_string(),
        };
        assert_eq!(launch.wire_kind(), "launch_failed");
        assert_eq!(launch.error(), Some("boom"));
        assert_eq!(launch.status(), None);

        let exited = TabRunEnding::ExitedWithStatus { status: 3 };
        assert_eq!(exited.wire_kind(), "exited");
        assert_eq!(exited.status(), Some(3));
        assert_eq!(exited.error(), None);

        assert_eq!(
            TabRunEnding::ExitedUnknownStatus.wire_kind(),
            "exited_unknown"
        );
        assert_eq!(TabRunEnding::RapidCleanExit.wire_kind(), "rapid_clean_exit");
    }
}
