//! THE content of the first-run welcome screen, so the TUI and the web UI say
//! identical words.
//!
//! This is prose, not layout: each surface decides how to frame, wrap, and
//! style it. Do NOT add a surface-local copy of any of this text.
//!
//! Distinct from [`crate::welcome`], which holds the rotating idle-pane TIPS.
//! This module is the one-time screen a brand new install sees.

use std::path::Path;

/// One numbered getting-started step.
///
/// Numbered because it genuinely is a sequence: you cannot create an agent
/// without a project, or launch one without an agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WelcomeStep {
    /// 1-based position, so a surface never has to derive it from an index.
    pub number: u8,
    pub title: &'static str,
    pub detail: &'static str,
}

/// Everything the welcome screen renders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WelcomeScreen {
    /// One bold line: what dux is.
    pub tagline: &'static str,
    /// Intro prose, one entry per paragraph. Owned because the last paragraph
    /// interpolates this machine's config path.
    pub paragraphs: Vec<String>,
    /// The numbered sequence. Deliberately repeats the prose so a reader who
    /// skips the paragraphs can still act.
    pub steps: &'static [WelcomeStep],
}

/// The one bold line at the top of the screen.
///
/// It names BOTH front ends on purpose. 0.7.0 is the release that announces the
/// web UI, and this screen is the first thing a brand new install shows, before
/// the user has any reason to visit the website: a headline that mentioned only
/// a terminal would teach them dux is a terminal-only product and they would
/// have no later prompt to unlearn it.
pub const TAGLINE: &str =
    "One git worktree per coding agent, watched from your terminal or your browser.";

pub const STEPS: &[WelcomeStep] = &[
    WelcomeStep {
        number: 1,
        title: "Add a project",
        detail: "Point dux at any git repo. Your checkout is left alone.",
    },
    WelcomeStep {
        number: 2,
        title: "Create an agent",
        detail: "It gets its own worktree and a branch-style name, so two agents never collide.",
    },
    WelcomeStep {
        number: 3,
        title: "Launch",
        detail: "Your provider CLI runs in a real terminal you can watch and type into, \
                 here or in a browser tab.",
    },
];

/// The welcome content for THIS machine.
///
/// `config_path` is passed in rather than resolved here so the function stays
/// pure and testable; callers hand it `DuxPaths::config_path`.
pub fn welcome_screen(config_path: &Path) -> WelcomeScreen {
    WelcomeScreen {
        tagline: TAGLINE,
        paragraphs: vec![
            "Start by adding a project: any git repo on this machine. Then create agents \
             on it. Each agent gets its own git worktree and a branch-style name, so they \
             work in parallel without ever tripping over each other's files."
                .to_string(),
            "Each agent runs whatever AI CLI you point it at. There is no protocol layer \
             and no adapter to write: if a tool runs in a terminal, it can be a provider."
                .to_string(),
            // The two-front-ends paragraph. Deliberately worded from NEITHER
            // surface's point of view (no "this terminal", no "this page"):
            // the same `WelcomeScreen` is projected into the web UI's bootstrap
            // (`viewmodel::BootstrapView::welcome_screen`), so a sentence that
            // assumed a terminal reader would be false in a browser and the
            // other way round. It follows the framing already used in README.md
            // and website/docs/introduction.md rather than inventing a third
            // description of the same thing.
            //
            // The "or flip a running terminal UI over to it" clause is load
            // bearing, not filler: one dux process owns the config directory
            // (see `crate::lockfile`), so a reader who took "run dux server" as
            // the only route would try it in a second shell alongside a running
            // TUI and hit the lock. The hand-off is the honest instruction.
            "dux has two front ends over one engine: a terminal UI and a web UI. Both are \
             first class, and they share the same projects, the same agents, the same \
             worktrees and the same config file, so an agent you start in one is the same \
             agent in the other. Start the web one with dux server, or flip a running \
             terminal UI over to it; either way the agents you left working carry on, now \
             reachable from your laptop or your phone."
                .to_string(),
            // `display()` is lossy for non-UTF-8 paths, which is the right
            // tradeoff for a sentence a human reads: the alternative is showing
            // nothing at all.
            format!(
                "Your config file lives at {}. It was written fully commented, every \
                 option explained where you change it. That file is the documentation.",
                config_path.display()
            ),
        ],
        steps: STEPS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn screen() -> WelcomeScreen {
        welcome_screen(&PathBuf::from("/home/ada/.config/dux/config.toml"))
    }

    #[test]
    fn the_steps_are_the_agreed_three_in_order() {
        let s = screen();
        assert_eq!(s.steps.len(), 3);
        assert_eq!(s.steps[0].title, "Add a project");
        assert_eq!(s.steps[1].title, "Create an agent");
        assert_eq!(s.steps[2].title, "Launch");
        // Numbers are carried, not derived, and they are 1-based and sequential.
        for (i, step) in s.steps.iter().enumerate() {
            assert_eq!(step.number as usize, i + 1);
        }
    }

    #[test]
    fn the_prose_covers_every_thing_a_new_user_must_learn() {
        let s = screen();
        let all = format!("{} {}", s.tagline, s.paragraphs.join(" ")).to_lowercase();
        for needle in ["project", "agent", "worktree", "branch", "cli"] {
            assert!(
                all.contains(needle),
                "the intro never mentions {needle}: {all}"
            );
        }
    }

    /// The screen a brand new install sees must teach that dux has TWO front
    /// ends, because it is seen before the website is and there is no second
    /// chance to correct a terminal-only first impression.
    ///
    /// Pinned as a PROPERTY (both surfaces named, plus the command that starts
    /// the web one) rather than as an exact sentence: the wording is expected to
    /// be reworked, and a literal-string assertion would fail on a rewrite that
    /// still says the right thing.
    #[test]
    fn the_screen_teaches_that_dux_has_two_front_ends_over_one_engine() {
        let s = screen();
        let all = format!("{} {}", s.tagline, s.paragraphs.join(" ")).to_lowercase();
        for needle in ["terminal", "browser", "web ui", "dux server"] {
            assert!(
                all.contains(needle),
                "the first-run screen never mentions {needle}, so a new user learns \
                 dux is terminal-only: {all}"
            );
        }
    }

    #[test]
    fn the_prose_names_this_machines_config_path_and_says_it_is_the_documentation() {
        // The path differs per platform (~/.dux on macOS, ~/.config/dux on
        // Linux), which is exactly why it is an input and not a constant.
        let s = welcome_screen(&PathBuf::from("/home/ada/.config/dux/config.toml"));
        let joined = s.paragraphs.join(" ");
        assert!(
            joined.contains("/home/ada/.config/dux/config.toml"),
            "the resolved config path must be shown verbatim: {joined}"
        );
        let mac = welcome_screen(&PathBuf::from("/Users/ada/.dux/config.toml"));
        assert!(
            mac.paragraphs
                .join(" ")
                .contains("/Users/ada/.dux/config.toml")
        );
        assert!(
            joined.to_lowercase().contains("documentation"),
            "the config file IS the documentation; say so: {joined}"
        );
    }

    #[test]
    fn a_multibyte_config_path_survives_intact() {
        // Home directories contain CJK and emoji in the wild; nothing here may
        // slice by byte offset.
        let path = PathBuf::from("/home/环境/📁 dux/config.toml");
        let s = welcome_screen(&path);
        assert!(
            s.paragraphs
                .join(" ")
                .contains("/home/环境/📁 dux/config.toml")
        );
    }

    #[test]
    fn there_are_intro_paragraphs_and_none_are_blank() {
        let s = screen();
        assert!(!s.paragraphs.is_empty(), "the screen needs some prose");
        for p in &s.paragraphs {
            assert_eq!(
                p.trim(),
                p.as_str(),
                "paragraph has stray whitespace: {p:?}"
            );
            assert!(!p.is_empty());
        }
    }

    #[test]
    fn the_step_details_repeat_the_prose_so_a_skimmer_can_still_act() {
        // Each step must stand on its own: a title plus a sentence of detail.
        for step in STEPS {
            assert!(!step.detail.is_empty(), "{} has no detail", step.title);
            assert!(
                step.detail.ends_with('.'),
                "{} detail should read as a sentence: {}",
                step.title,
                step.detail
            );
        }
    }
}
