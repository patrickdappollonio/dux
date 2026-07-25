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

pub const TAGLINE: &str =
    "One git worktree per coding agent, and a real terminal to watch it work.";

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
        detail: "Your provider CLI runs in a real terminal you can watch and type into.",
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
