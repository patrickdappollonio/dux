//! THE single source of truth for welcome-screen tips, shared by every surface.
//!
//! Both the TUI's idle agent pane and the web UI's center-pane welcome screen
//! render from this one list — do NOT add surface-local tip lists. To add a
//! tip, append a `WelcomeTip` here and provide BOTH renderings when the
//! feature exists on both surfaces (`web: None` / `tui: None` mark a tip as
//! surface-specific). Keep the tone playful and sassy (see CLAUDE.md's
//! "Welcome tips" tenet): lead with the feature discovery, keep key/button
//! references secondary. Wrap text in backticks to highlight it in an accent
//! color on both surfaces (the backticks themselves are never rendered).
//! TUI variants receive a resolver so keybinding labels stay accurate after
//! rebinding — never hardcode key names.

use crate::action::Action;

/// Resolves an [`Action`] to its current keybinding label (e.g. "Ctrl-g").
/// Implemented by the TUI's `RuntimeBindings`; core stays keybinding-agnostic.
pub type KeyLabelResolver<'a> = &'a dyn Fn(Action) -> String;

pub struct WelcomeTip {
    /// Web rendering. Keybinding-free — reference web affordances (⌘K, buttons,
    /// the ⋯ menu). `None` = TUI-only tip.
    pub web: Option<&'static str>,
    /// TUI rendering, given the live key-label resolver. `None` = web-only tip.
    pub tui: Option<fn(KeyLabelResolver) -> String>,
}

pub const WELCOME_TIPS: &[WelcomeTip] = &[
    // --- shared: feature exists on both surfaces ---
    WelcomeTip {
        web: Some(
            "Lost? `⌘K` opens the command palette. Every action lives there, even the ones you forgot existed.",
        ),
        tui: Some(|resolve| {
            format!(
                "Lost? `{}` opens the command palette. Every action lives there, even the ones you forgot existed.",
                resolve(Action::OpenPalette)
            )
        }),
    },
    WelcomeTip {
        web: Some(
            "Need every keystroke? The `fullscreen` button on a terminal captures even `Ctrl+T`. Focus mode: activated.",
        ),
        tui: Some(|resolve| {
            format!(
                "Need more room? `{}` toggles interactive mode, going fullscreen. Focus mode: activated.",
                resolve(Action::ExitInteractive)
            )
        }),
    },
    WelcomeTip {
        web: Some("Every project's `⋯` menu can spawn a `New agent…`. The more, the merrier."),
        tui: Some(|resolve| {
            format!(
                "`{}` spawns a new agent in the current worktree. The more, the merrier.",
                resolve(Action::NewAgent)
            )
        }),
    },
    WelcomeTip {
        web: Some(
            "Any CLI tool can be a provider. Just set its `command` in config.toml. No plugins, no adapters.",
        ),
        tui: Some(|_resolve| {
            "Any CLI tool can be a provider. Just set its `command` in config.toml. No plugins, no adapters.".into()
        }),
    },
    WelcomeTip {
        web: Some("Each agent gets companion terminals. The `⋯` menu spawns as many as you like."),
        tui: Some(|resolve| {
            format!(
                "`{}` flips between agent and companion terminal. Two views, one worktree.",
                resolve(Action::ShowTerminal)
            )
        }),
    },
    WelcomeTip {
        web: Some("Hover a changed file and hit `Stage`. Git add, minus the typing."),
        tui: Some(|resolve| {
            format!(
                "`{}` stages or unstages the selected file. Git add, minus the typing.",
                resolve(Action::StageUnstage)
            )
        }),
    },
    WelcomeTip {
        web: Some(
            "Tired of writing commit messages? `Generate with AI` in the commit dialog does it for you.",
        ),
        tui: Some(|resolve| {
            format!(
                "Tired of writing commit messages? `{}` lets AI do it for you.",
                resolve(Action::GenerateCommitMessage)
            )
        }),
    },
    WelcomeTip {
        web: None,
        tui: Some(|resolve| {
            format!(
                "`{}` forks the current agent into a brand new session. Cloning never felt so good.",
                resolve(Action::ForkAgent)
            )
        }),
    },
    WelcomeTip {
        web: None,
        tui: Some(|resolve| {
            format!(
                "`{}` and `{}` hop between panes. Tab your way through everything.",
                resolve(Action::FocusNext),
                resolve(Action::FocusPrev)
            )
        }),
    },
    WelcomeTip {
        web: None,
        tui: Some(|resolve| {
            format!(
                "Open the palette with `{}` and run `change-agent-provider` to swap a worktree's CLI. Been here before? dux resumes that provider's last session automatically.",
                resolve(Action::OpenPalette)
            )
        }),
    },
    WelcomeTip {
        web: Some(
            "dux remembers which providers you've run on each worktree. Swap away and back, and each one picks up right where you left it.",
        ),
        tui: Some(|_resolve| {
            "dux remembers which providers you've run on each worktree. Swap away and back, and each one picks up right where you left it.".into()
        }),
    },
    WelcomeTip {
        web: Some(
            "A project's settings (the `⋯` menu) can pin its own default provider. One project, one brain.",
        ),
        tui: Some(|_resolve| {
            "Need to change which CLI new agents use? `change-default-provider` updates the global fallback. `change-project-default-provider` overrides just one project.".into()
        }),
    },
    WelcomeTip {
        web: None,
        tui: Some(|_resolve| {
            "Swapped providers while an agent was still running? The sidebar shows `(old → new)` until you exit and relaunch. dux queues the swap, you run the show.".into()
        }),
    },
    WelcomeTip {
        web: None,
        tui: Some(|_resolve| {
            "The mouse works everywhere: click panes, scroll output, select files. Go ahead, click around.".into()
        }),
    },
    WelcomeTip {
        web: Some("Drag the sidebar's right edge to resize it. It remembers."),
        tui: Some(|_resolve| {
            "Drag pane borders with the mouse to resize them. No keybindings required.".into()
        }),
    },
    // Merged with the ShowTerminal flip tip above (web variant lives there) to
    // avoid two near-duplicate web entries about companion terminals.
    WelcomeTip {
        web: None,
        tui: Some(|resolve| {
            format!(
                "Each agent gets its own companion terminal. Press `{}` to spawn more than one.",
                resolve(Action::ShowTerminal)
            )
        }),
    },
    WelcomeTip {
        web: None,
        tui: Some(|resolve| {
            format!(
                "Don't need the git pane? `{}` hides it. Want it gone for good? Check the command palette.",
                resolve(Action::ToggleGitPane)
            )
        }),
    },
    WelcomeTip {
        web: None,
        tui: Some(|resolve| {
            format!(
                "The `{}` key toggles the left sidebar. Maximum screen real estate, minimum distractions.",
                resolve(Action::ToggleSidebar)
            )
        }),
    },
    WelcomeTip {
        web: None,
        tui: Some(|_resolve| {
            "Every keybinding is configurable. Open config.toml and make dux truly yours.".into()
        }),
    },
    WelcomeTip {
        web: None,
        tui: Some(|_resolve| {
            "Worktrees are the secret sauce: each agent gets its own isolated branch. No conflicts, ever.".into()
        }),
    },
    WelcomeTip {
        web: None,
        tui: Some(|resolve| {
            format!(
                "`{}` opens the project browser. Add worktrees from anywhere on disk.",
                resolve(Action::OpenProjectBrowser)
            )
        }),
    },
    WelcomeTip {
        web: None,
        tui: Some(|resolve| {
            format!(
                "`{}` opens the help overlay, the full keybinding reference, right in the app.",
                resolve(Action::ToggleHelp)
            )
        }),
    },
    WelcomeTip {
        web: None,
        tui: Some(|resolve| {
            format!(
                "Macros let you save and replay prompts. Configure them in config.toml, trigger with `{}`.",
                resolve(Action::OpenMacroBar)
            )
        }),
    },
    WelcomeTip {
        web: None,
        tui: Some(|_resolve| {
            "Launch 5 agents on 5 worktrees and let them all work in parallel. Conflicts? Let AI sort it out.".into()
        }),
    },
    WelcomeTip {
        web: None,
        tui: Some(|_resolve| {
            "Tired of typing the same prompt to your AI agent over and over? Turn it into a macro!"
                .into()
        }),
    },
    WelcomeTip {
        web: None,
        tui: Some(|_resolve| {
            "Dux runs Claude the way Anthropic intended. No workarounds, no bans. Just vibes."
                .into()
        }),
    },
    WelcomeTip {
        web: None,
        tui: Some(|_resolve| {
            "The config file is also the documentation. Every option is configurable and the comments explain it all.".into()
        }),
    },
    WelcomeTip {
        web: None,
        tui: Some(|_resolve| {
            "Curious what you changed in your config? Run `dux config diff` to see exactly what's different from the defaults.".into()
        }),
    },
    WelcomeTip {
        web: None,
        tui: Some(|resolve| {
            format!(
                "Agent keybinds clashing with dux? `{}` toggles interactive mode. Most keys go straight to the agent.",
                resolve(Action::ExitInteractive)
            )
        }),
    },
    WelcomeTip {
        web: None,
        tui: Some(|_resolve| {
            "New agent prompt looking too empty? Tick the pet-name checkbox and let dux name your next chaos gremlin.".into()
        }),
    },
    WelcomeTip {
        web: None,
        tui: Some(|_resolve| {
            "Install the `gh` CLI and your agents can create commits and pull requests. Pair it with macros or skills to match your style.".into()
        }),
    },
    WelcomeTip {
        web: None,
        tui: Some(|_resolve| {
            "Your MCP servers, tools, and hooks? They all just work. We don't mess with your setup. Promise.".into()
        }),
    },
    WelcomeTip {
        web: None,
        tui: Some(|resolve| {
            format!(
                "Terminal looking glitchy? `{}` redraws the entire screen. Good as new.",
                resolve(Action::ForceRedraw)
            )
        }),
    },
    WelcomeTip {
        web: None,
        tui: Some(|resolve| {
            format!(
                "The command palette (`{}`) has features that don't have keybinds. Poke around, you might be surprised.",
                resolve(Action::OpenPalette)
            )
        }),
    },
    // --- web-only additions (no TUI equivalent) ---
    WelcomeTip {
        web: Some(
            "Click a changed file to read its diff — syntax highlighting included, no checkout required.",
        ),
        tui: None,
    },
    WelcomeTip {
        web: Some(
            "Agents keep running when you close this tab. Come back any time; the terminal repaints like you never left.",
        ),
        tui: None,
    },
    WelcomeTip {
        web: Some(
            "Hover an agent's status icon to see how it's doing: green runs, amber waits, gray is gone.",
        ),
        tui: None,
    },
];

/// The web-surface tip strings, in declaration order.
pub fn web_tips() -> Vec<String> {
    WELCOME_TIPS
        .iter()
        .filter_map(|t| t.web.map(str::to_string))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tip_has_at_least_one_variant() {
        for (i, tip) in WELCOME_TIPS.iter().enumerate() {
            assert!(
                tip.web.is_some() || tip.tui.is_some(),
                "tip {i} has no rendering for any surface"
            );
        }
    }

    #[test]
    fn every_present_variant_has_balanced_backticks() {
        let resolve = |a: Action| format!("{a:?}");
        for (i, tip) in WELCOME_TIPS.iter().enumerate() {
            if let Some(web) = tip.web {
                assert_eq!(
                    web.matches('`').count() % 2,
                    0,
                    "web variant of tip {i} has unbalanced backticks: {web}"
                );
            }
            if let Some(tui) = tip.tui {
                let rendered = tui(&resolve);
                assert_eq!(
                    rendered.matches('`').count() % 2,
                    0,
                    "tui variant of tip {i} has unbalanced backticks: {rendered}"
                );
            }
        }
    }

    #[test]
    fn web_tips_is_non_empty() {
        assert!(!web_tips().is_empty());
    }
}
