//! The first-load gate: which of the two first-load screens (if either) a
//! launch should show, and whether that launch should record the running
//! version as seen.
//!
//! Pure and shared. Both surfaces call [`plan`] at startup and [`after_fetch`]
//! once the release-notes worker returns; neither reimplements the rules.
//!
//! The last-seen version lives in SQLite (`SessionStore::last_seen_version`) and
//! is therefore SHARED: dismissing the what's-new screen in the TUI dismisses it
//! in the web UI too. There is deliberately no per-browser or config-based
//! alternative.
//!
//! # WHEN to stamp the version — the contract both surfaces must honor
//!
//! [`FirstLoadPlan::mark_seen`] says *whether* to write `last_seen_version`. It
//! deliberately does not say *when*, so here is the rule, and it is not
//! negotiable per surface:
//!
//! - **`screen != Nothing`** — hold the plan in memory and stamp **when the user
//!   dismisses the screen**, NOT when the plan is computed. The web server is
//!   long-lived: if it stamped at startup, a browser that connected a minute
//!   later would find the version already seen and show nothing at all.
//! - **`screen == Nothing && mark_seen`** — stamp **immediately**. There is no
//!   screen to dismiss, so there is nothing to wait for.
//!
//! Because the value is one shared SQLite row, dismissing on either surface
//! settles it for both.

/// The literal `DUX_DISPLAY_VERSION` of any build without `DUX_RELEASE_BUILD=1`.
pub const DEVELOPMENT_VERSION: &str = "development";

/// Which first-load screen to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirstLoad {
    /// Show neither screen.
    Nothing,
    /// First ever launch: no version has been recorded.
    Welcome,
    /// The recorded version differs from the running one.
    WhatsNew,
}

/// What a launch should do: the screen, plus whether to stamp the running
/// version as seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirstLoadPlan {
    pub screen: FirstLoad,
    /// Whether the running version should be written to `last_seen_version`.
    ///
    /// True when a screen is actually going on the display (so it does not come
    /// back next launch) and also when a screen was suppressed by config (the
    /// user opted out, so keep the state moving forward rather than pinning them
    /// at the old version forever). False when nothing changed, when the build is
    /// a development build, and — critically — when the release-notes fetch
    /// failed TRANSIENTLY, so the notes appear on a later launch that has network.
    ///
    /// See the module docs for WHEN to act on this.
    pub mark_seen: bool,
}

/// How the release-notes fetch ended. Three outcomes, not two, because "GitHub
/// says no release exists for this tag" and "dux could not reach GitHub" demand
/// opposite treatment of [`FirstLoadPlan::mark_seen`], and a `bool` lets a caller
/// conflate them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotesOutcome {
    /// The notes are in hand.
    Fetched,
    /// GitHub answered definitively: there is no release for this tag (404).
    /// Common and legitimate — a locally built or not-yet-published tagged
    /// binary has no release page. Retrying can never help, so this DOES mark
    /// the version seen; otherwise every future launch would repeat a request
    /// whose answer cannot change.
    NoSuchRelease,
    /// Something that might work later: offline, timeout, 5xx, or a rate limit.
    /// Does NOT mark the version seen, so the notes get another chance.
    TemporarilyUnavailable,
}

impl FirstLoadPlan {
    const NOTHING: Self = Self {
        screen: FirstLoad::Nothing,
        mark_seen: false,
    };
}

/// STEP 1, no network: what the stored state and the config say should happen.
///
/// - `last_seen`: the version recorded in SQLite, `None` on a first ever launch.
/// - `running`: the running build's display version (`"development"` for a
///   non-release build).
/// - `disable_welcome` / `disable_release_notes`: the two `[ui]` flags. Each
///   suppresses only the AUTOMATIC showing; an explicit user action to open
///   either screen bypasses this function entirely.
pub fn plan(
    last_seen: Option<&str>,
    running: &str,
    disable_welcome: bool,
    disable_release_notes: bool,
) -> FirstLoadPlan {
    let Some(last_seen) = last_seen else {
        // First ever launch. A development build DOES get the welcome (a fresh
        // dev install is still a fresh install, and the welcome needs no
        // network and names no version), and stamps "development" so it does not
        // reappear on every dev launch.
        return if disable_welcome {
            FirstLoadPlan {
                screen: FirstLoad::Nothing,
                mark_seen: true,
            }
        } else {
            FirstLoadPlan {
                screen: FirstLoad::Welcome,
                mark_seen: true,
            }
        };
    };

    if last_seen == running {
        return FirstLoadPlan::NOTHING;
    }

    // A development build never auto-shows release notes: there is no release to
    // show notes for. It also never stamps, so running a dev build in between two
    // real versions cannot swallow the real version's notes.
    if running == DEVELOPMENT_VERSION {
        return FirstLoadPlan::NOTHING;
    }

    if disable_release_notes {
        return FirstLoadPlan {
            screen: FirstLoad::Nothing,
            mark_seen: true,
        };
    }

    FirstLoadPlan {
        screen: FirstLoad::WhatsNew,
        mark_seen: true,
    }
}

/// STEP 2, after the release-notes worker returns: folds the fetch outcome in.
///
/// Only [`FirstLoad::WhatsNew`] depends on the network; every other plan passes
/// through unchanged (the welcome screen needs no notes).
///
/// - [`NotesOutcome::Fetched`] — keep the screen and the stamp.
/// - [`NotesOutcome::NoSuchRelease`] — no screen, but DO stamp: the answer is
///   definitive and re-asking every launch is a permanent pointless request.
/// - [`NotesOutcome::TemporarilyUnavailable`] — no screen and NO stamp, so the
///   notes reappear on a launch that can reach GitHub.
pub fn after_fetch(plan: FirstLoadPlan, outcome: NotesOutcome) -> FirstLoadPlan {
    if plan.screen != FirstLoad::WhatsNew {
        return plan;
    }
    match outcome {
        NotesOutcome::Fetched => plan,
        NotesOutcome::NoSuchRelease => FirstLoadPlan {
            screen: FirstLoad::Nothing,
            mark_seen: true,
        },
        NotesOutcome::TemporarilyUnavailable => FirstLoadPlan::NOTHING,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V6: &str = "v0.6.0";
    const V7: &str = "v0.7.0";

    #[test]
    fn no_stored_version_shows_the_welcome_and_stamps_it() {
        let p = plan(None, V6, false, false);
        assert_eq!(p.screen, FirstLoad::Welcome);
        assert!(p.mark_seen, "the welcome must not reappear next launch");
    }

    #[test]
    fn a_newer_running_version_shows_whats_new() {
        let p = plan(Some(V6), V7, false, false);
        assert_eq!(p.screen, FirstLoad::WhatsNew);
        assert!(p.mark_seen);
    }

    #[test]
    fn a_downgrade_also_counts_as_a_change() {
        // The rule is "differs", not "is newer": there is no version comparison
        // and only the newest release is ever fetched.
        assert_eq!(plan(Some(V7), V6, false, false).screen, FirstLoad::WhatsNew);
    }

    #[test]
    fn the_same_version_shows_nothing_and_writes_nothing() {
        assert_eq!(plan(Some(V6), V6, false, false), FirstLoadPlan::NOTHING);
    }

    #[test]
    fn a_development_build_never_auto_shows_release_notes() {
        let p = plan(Some(V6), DEVELOPMENT_VERSION, false, false);
        assert_eq!(p.screen, FirstLoad::Nothing);
        assert!(
            !p.mark_seen,
            "stamping 'development' over v0.6.0 would swallow a real release's notes"
        );
    }

    #[test]
    fn a_development_build_still_gets_the_first_run_welcome() {
        let p = plan(None, DEVELOPMENT_VERSION, false, false);
        assert_eq!(p.screen, FirstLoad::Welcome);
        assert!(p.mark_seen, "otherwise it reappears on every dev launch");
    }

    #[test]
    fn a_development_build_that_already_saw_the_welcome_shows_nothing() {
        assert_eq!(
            plan(Some(DEVELOPMENT_VERSION), DEVELOPMENT_VERSION, false, false),
            FirstLoadPlan::NOTHING
        );
    }

    #[test]
    fn disable_automated_welcome_screen_suppresses_only_the_welcome() {
        let p = plan(None, V6, true, false);
        assert_eq!(p.screen, FirstLoad::Nothing);
        assert!(
            p.mark_seen,
            "stamp anyway, so a later upgrade still gets its what's-new screen"
        );
        // Proof of that last claim: with the version now stamped, an upgrade
        // shows the notes even though the welcome stayed off.
        assert_eq!(plan(Some(V6), V7, true, false).screen, FirstLoad::WhatsNew);
    }

    #[test]
    fn disable_release_notes_suppresses_only_the_whats_new_screen() {
        let p = plan(Some(V6), V7, false, true);
        assert_eq!(p.screen, FirstLoad::Nothing);
        assert!(
            p.mark_seen,
            "the user opted out; do not re-decide every launch"
        );
        // The welcome is unaffected by the release-notes flag.
        assert_eq!(plan(None, V6, false, true).screen, FirstLoad::Welcome);
    }

    #[test]
    fn both_flags_set_shows_nothing_on_either_path() {
        assert_eq!(plan(None, V6, true, true).screen, FirstLoad::Nothing);
        assert_eq!(plan(Some(V6), V7, true, true).screen, FirstLoad::Nothing);
    }

    #[test]
    fn a_transient_fetch_failure_shows_nothing_and_leaves_the_version_unmarked() {
        // THE subtle rule: an offline launch must not consume the notes.
        let p = after_fetch(
            plan(Some(V6), V7, false, false),
            NotesOutcome::TemporarilyUnavailable,
        );
        assert_eq!(p.screen, FirstLoad::Nothing);
        assert!(
            !p.mark_seen,
            "marking seen here would hide v0.7.0's notes forever"
        );
    }

    #[test]
    fn a_definitive_404_shows_nothing_but_does_mark_the_version_seen() {
        // A locally built or unpublished tagged binary has no release page.
        // Re-asking every launch is a permanent pointless request, so unlike a
        // transient failure this one settles the version.
        let p = after_fetch(
            plan(Some(V6), V7, false, false),
            NotesOutcome::NoSuchRelease,
        );
        assert_eq!(p.screen, FirstLoad::Nothing);
        assert!(p.mark_seen, "a definitive answer must not be re-asked");
        // Proof it settles: the next launch decides nothing at all.
        assert_eq!(plan(Some(V7), V7, false, false), FirstLoadPlan::NOTHING);
    }

    #[test]
    fn a_successful_fetch_keeps_the_whats_new_plan() {
        let p = after_fetch(plan(Some(V6), V7, false, false), NotesOutcome::Fetched);
        assert_eq!(p.screen, FirstLoad::WhatsNew);
        assert!(p.mark_seen);
    }

    #[test]
    fn the_two_failure_outcomes_are_never_interchangeable() {
        // The whole reason `NotesOutcome` is not a bool.
        let base = plan(Some(V6), V7, false, false);
        assert_ne!(
            after_fetch(base, NotesOutcome::NoSuchRelease),
            after_fetch(base, NotesOutcome::TemporarilyUnavailable)
        );
    }

    #[test]
    fn after_fetch_never_alters_a_non_network_plan() {
        // The welcome needs no network, and a suppressed plan must keep its
        // stamp regardless of what the (unmade) fetch would have returned.
        for outcome in [
            NotesOutcome::Fetched,
            NotesOutcome::NoSuchRelease,
            NotesOutcome::TemporarilyUnavailable,
        ] {
            assert_eq!(
                after_fetch(plan(None, V6, false, false), outcome),
                plan(None, V6, false, false)
            );
            assert_eq!(
                after_fetch(plan(None, V6, true, false), outcome),
                plan(None, V6, true, false)
            );
            assert_eq!(
                after_fetch(plan(Some(V6), V7, false, true), outcome),
                plan(Some(V6), V7, false, true)
            );
            assert_eq!(
                after_fetch(plan(Some(V6), V6, false, false), outcome),
                FirstLoadPlan::NOTHING
            );
        }
    }

    /// Every combination of the inputs, so no case is left undecided.
    #[test]
    fn the_decision_table_is_exhaustive_and_total() {
        let versions = [None, Some(V6), Some(V7), Some(DEVELOPMENT_VERSION)];
        let running = [V6, V7, DEVELOPMENT_VERSION];
        for last in versions {
            for run in running {
                for dw in [false, true] {
                    for drn in [false, true] {
                        let p = plan(last, run, dw, drn);
                        // A dev build never lands on WhatsNew, whatever the inputs.
                        if run == DEVELOPMENT_VERSION {
                            assert_ne!(
                                p.screen,
                                FirstLoad::WhatsNew,
                                "dev build showed release notes for {last:?}/{run}"
                            );
                        }
                        // A screen that is shown always stamps, or it would loop.
                        if p.screen != FirstLoad::Nothing {
                            assert!(p.mark_seen, "{last:?}/{run}/{dw}/{drn} would loop");
                        }
                        // Nothing-to-do never writes.
                        if last == Some(run) {
                            assert_eq!(p, FirstLoadPlan::NOTHING);
                        }
                    }
                }
            }
        }
    }
}
