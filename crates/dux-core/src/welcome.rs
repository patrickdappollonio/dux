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
    /// Web rendering. Keybinding-free: reference web affordances (the cog app
    /// menu, buttons, the ⋯ menu). The web has NO command palette and no
    /// keyboard shortcut for its menu, so never point a web tip at one.
    /// `None` = TUI-only tip.
    pub web: Option<&'static str>,
    /// TUI rendering, given the live key-label resolver. `None` = web-only tip.
    pub tui: Option<fn(KeyLabelResolver) -> String>,
}

pub const WELCOME_TIPS: &[WelcomeTip] = &[
    // --- shared tips, plus the TUI-only ones (`web: None`) whose feature has
    // no web counterpart at all: a key-driven fullscreen, the help overlay,
    // pane hopping, the sidebar key, screen redraw, the palette itself ---
    WelcomeTip {
        web: Some(
            "Lost? The `cog` up top opens the app menu. Preferences, config, macros, the lot, no keyboard archaeology required.",
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
            "Need more room? `Theater mode` hands the whole screen to the terminal, and a floating pill keeps the way back. Focus mode: activated.",
        ),
        tui: Some(|resolve| {
            format!(
                "Need more room? `{}` toggles the agent pane fullscreen. Focus mode: activated.",
                resolve(Action::ToggleFullscreen)
            )
        }),
    },
    WelcomeTip {
        web: Some(
            "Hit `New agent`, search any project, pick a provider, and go. The more, the merrier.",
        ),
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
            "An agent's `⋯` menu carries `Fork agent…`, which clones the whole session into a brand new one. Cloning never felt so good.",
        ),
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
        web: Some(
            "`Change agent provider…` in an agent's `⋯` menu swaps the CLI on that worktree. Been here before? dux resumes that provider's last session automatically.",
        ),
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
            "In the `New agent` picker, each project's `⋯` menu can pin its own default provider. One project, one brain.",
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
        web: Some(
            "Don't need the Changes pane? `Hide Changes pane` in its `⋯` menu tucks it away, and a button in the header brings it back.",
        ),
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
        web: Some(
            "Worktrees are the secret sauce: an agent in a project gets its own isolated branch. No conflicts, ever.",
        ),
        tui: Some(|_resolve| {
            "Worktrees are the secret sauce: an agent in a project gets its own isolated branch. No conflicts, ever.".into()
        }),
    },
    // The standalone star: one indicator, learned once, on both surfaces.
    WelcomeTip {
        web: Some(
            "Spot a ✷ in the sidebar? Standalone. That agent or terminal lives in your own folder, not a worktree dux made. Your folder, your rules.",
        ),
        tui: Some(|_resolve| {
            "Spot a ✷ in the sidebar? Standalone. That agent or terminal lives in your own folder, not a worktree dux made. Your folder, your rules.".into()
        }),
    },
    WelcomeTip {
        web: Some(
            "`Add project…` in the launcher's `⋯` menu browses the server's own disk, so you can adopt a repo from a phone on the sofa.",
        ),
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
        web: Some(
            "Macros let you save and replay prompts. The floating `Macros…` button drops one into the terminal without sending it, so you still get the last word.",
        ),
        tui: Some(|resolve| {
            format!(
                "Macros let you save and replay prompts. Configure them in config.toml, trigger with `{}`.",
                resolve(Action::OpenMacroBar)
            )
        }),
    },
    WelcomeTip {
        web: Some(
            "Launch 5 agents on 5 worktrees and let them all work in parallel. Conflicts? Let AI sort it out.",
        ),
        tui: Some(|_resolve| {
            "Launch 5 agents on 5 worktrees and let them all work in parallel. Conflicts? Let AI sort it out.".into()
        }),
    },
    WelcomeTip {
        web: Some(
            "Tired of typing the same prompt to your AI agent over and over? Turn it into a macro. `Edit macros…` sits in the cog menu, under `Configuration`.",
        ),
        tui: Some(|_resolve| {
            "Tired of typing the same prompt to your AI agent over and over? Turn it into a macro!"
                .into()
        }),
    },
    WelcomeTip {
        web: Some(
            "Dux runs Claude the way Anthropic intended. No workarounds, no bans. Just vibes.",
        ),
        tui: Some(|_resolve| {
            "Dux runs Claude the way Anthropic intended. No workarounds, no bans. Just vibes."
                .into()
        }),
    },
    WelcomeTip {
        web: Some(
            "The config file is also the documentation, and `Edit config file…` in the cog menu's `Configuration` group opens it right here. Every option is configurable and the comments explain it all.",
        ),
        tui: Some(|_resolve| {
            "The config file is also the documentation. Every option is configurable and the comments explain it all.".into()
        }),
    },
    WelcomeTip {
        web: Some(
            "Curious what you changed in your config? `dux config diff` in a shell on the server spells out exactly what's different from the defaults, and it's safe to paste into a bug report.",
        ),
        tui: Some(|_resolve| {
            "Curious what you changed in your config? Run `dux config diff` to see exactly what's different from the defaults.".into()
        }),
    },
    WelcomeTip {
        web: None,
        tui: Some(|resolve| {
            format!(
                "Agent keybinds clashing with dux? `{}` goes fullscreen, where keys reach the agent verbatim.",
                resolve(Action::ToggleFullscreen)
            )
        }),
    },
    WelcomeTip {
        web: Some(
            "Leave the branch name blank in `New agent…` and dux names your next chaos gremlin for you.",
        ),
        tui: Some(|_resolve| {
            "New agent prompt looking too empty? Tick the pet-name checkbox and let dux name your next chaos gremlin.".into()
        }),
    },
    WelcomeTip {
        web: Some(
            "Install the `gh` CLI and your agents can create commits and open pull requests themselves. dux spots the PR and hangs a banner over the terminal.",
        ),
        tui: Some(|_resolve| {
            "Install the `gh` CLI and your agents can create commits and pull requests. Pair it with macros or skills to match your style.".into()
        }),
    },
    WelcomeTip {
        web: Some(
            "Your MCP servers, tools, and hooks? They all just work. We don't mess with your setup. Promise.",
        ),
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
    WelcomeTip {
        web: Some(
            "One agent can run several provider sessions at once on the very same worktree. The `+` on the tab strip adds one, and its dropdown picks a different CLI.",
        ),
        tui: Some(|resolve| {
            format!(
                "One agent, several provider sessions, one worktree. `new-agent-tab` in the palette adds a tab and `{}` walks the strip.",
                resolve(Action::NextTab)
            )
        }),
    },
    WelcomeTip {
        web: Some(
            "In theater mode the floating pill is yours to drag, so park the way out wherever your thumb already lives.",
        ),
        tui: Some(|resolve| {
            format!(
                "Fullscreen (`{}`) doesn't draw the tab strip at all, so hop tabs before you dive in.",
                resolve(Action::ToggleFullscreen)
            )
        }),
    },
    WelcomeTip {
        web: Some(
            "Only one device types at a time. When somebody else is driving, a card names them and `Take over` hands you the keyboard, with nothing lost.",
        ),
        tui: Some(|resolve| {
            format!(
                "The terminal UI is just another device in the queue: when a browser is driving, a card says so and `{}` presses its `Take over` button on the focused pane.",
                resolve(Action::FocusAgent)
            )
        }),
    },
    WelcomeTip {
        web: Some(
            "No worktree, no branch, just a folder you already have: `New standalone agent…` in the launcher's `⋯` menu. dux never creates, moves or removes that folder.",
        ),
        tui: Some(|resolve| {
            format!(
                "`new-standalone-agent` in the palette (`{}`) starts an agent in a folder you already have, worktree not included. dux never creates, moves or removes that folder.",
                resolve(Action::OpenPalette)
            )
        }),
    },
    WelcomeTip {
        web: Some(
            "Need a plain shell before you've added a single project? The `+` on the `Terminals` divider opens one in your home directory, owned by nothing at all.",
        ),
        tui: Some(|_resolve| {
            "Need a plain shell before you've added a single project? `new-standalone-terminal` in the palette opens one in your home directory, owned by nothing at all.".into()
        }),
    },
    WelcomeTip {
        web: Some(
            "The Changes pane totals the green and red lines for every group and for the pane itself, and a big sum reads in thousands, rounded down so it never oversells.",
        ),
        tui: Some(|_resolve| {
            "The changes pane totals the green and red lines for every group and for the pane itself, and a big sum reads in thousands, rounded down so it never oversells.".into()
        }),
    },
    WelcomeTip {
        web: Some(
            "An agent tied to a pull request wears a one-line banner with the PR's number, state and title, color-coded, and clicking it opens the PR in a new tab.",
        ),
        tui: Some(|resolve| {
            format!(
                "An agent tied to a pull request wears a one-line banner with the PR's number, state and title, and `{}` opens it in your browser.",
                resolve(Action::OpenCurrentPullRequest)
            )
        }),
    },
    WelcomeTip {
        web: Some(
            "Stage what you want, then `Commit…`, `Push` and `Pull` in the Changes pane's `⋯` menu do the everyday git chores. No shell required.",
        ),
        tui: Some(|resolve| {
            format!(
                "Stage what you want, `{}` writes the commit and `{}` pushes it. No shell required.",
                resolve(Action::CommitChanges),
                resolve(Action::PushToRemote)
            )
        }),
    },
    WelcomeTip {
        web: Some(
            "Upgraded and wondering what landed? The cog menu keeps `What's new…` and `Welcome screen…` around long after you dismissed them.",
        ),
        tui: Some(|resolve| {
            format!(
                "Upgraded and wondering what landed? `show-release-notes` in the palette (`{}`) reopens the notes, and `show-welcome-screen` brings the duck back.",
                resolve(Action::OpenPalette)
            )
        }),
    },
    WelcomeTip {
        web: Some(
            "Click a changed file to read its diff, syntax highlighting included, no checkout required.",
        ),
        tui: Some(|resolve| {
            format!(
                "`{}` opens the selected file's diff, syntax highlighting included, no checkout required.",
                resolve(Action::OpenDiff)
            )
        }),
    },
    // --- web-only additions (no TUI equivalent) ---
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
    // Web-only on purpose: the browser is where dux edits files, and the TUI's
    // answer is to open the worktree in your own editor instead.
    WelcomeTip {
        web: Some(
            "Faster to fix the typo than to explain it? `Open editor` in the Changes pane header opens a real editor over the worktree, file tree and all.",
        ),
        tui: None,
    },
    // Web-only on purpose: in the terminal UI, a dropped file is your terminal
    // emulator's business, not dux's.
    WelcomeTip {
        web: Some(
            "Drag a screenshot onto the terminal and dux saves it on the server and pastes its path into the prompt, ready for you to finish the sentence.",
        ),
        tui: None,
    },
    // Web-only on purpose: there is no soft keyboard in front of the TUI.
    WelcomeTip {
        web: Some(
            "On a phone the message box under the terminal is where autocorrect, swipe typing and voice input actually work. `Send` delivers the lot and presses Enter for you.",
        ),
        tui: None,
    },
    // --- shared again: serving dux over the web, from both sides ---
    // The REACH tip: what a second front end BUYS you. Deliberately mechanism-
    // light on both sides, because the tip right below already teaches the how
    // (`start-web-server`), and because naming `dux server` here would be a
    // half-truth in the TUI: one dux process owns the config directory
    // (`crate::lockfile`), so a reader who ran it in a second shell alongside
    // their running TUI would meet the lock rather than their agents.
    WelcomeTip {
        web: Some(
            "That agent grinding away in your terminal? It's this one. Same worktree, same branch, one engine wearing two faces.",
        ),
        tui: Some(|_resolve| {
            "Your agents don't live in this window, they live in worktrees. Serve dux over the web and the very same sessions turn up on your phone, mid-thought. No protocol layer, no adapter.".into()
        }),
    },
    // The in-process flip: discoverable from both sides. The web variant winks
    // at how the user may have gotten here; the TUI variant advertises the way
    // out. `start-web-server` is a palette-only command (no keybinding), so
    // naming it literally is correct — there is no label to resolve.
    WelcomeTip {
        web: Some(
            "This whole web UI can be born from the terminal UI: the `start-web-server` command flips dux inside out, and the agents never notice.",
        ),
        tui: Some(|resolve| {
            format!(
                "dux has a second face: `start-web-server` in the palette (`{}`) serves this whole thing to your browser. Your agents keep purring through the swap.",
                resolve(Action::OpenPalette)
            )
        }),
    },
    // The BACKGROUND mode, next to the flip on purpose: they are the two answers
    // to the same question and the pair reads as a choice. Both names are
    // palette-only commands with no keybinding, so naming them literally is
    // correct; the palette's own key is resolved.
    WelcomeTip {
        web: Some(
            "Someone may be sitting in the terminal UI this very second: `serve_while_tui` runs both faces at once. First one to type drives, everybody else gets front-row seats.",
        ),
        tui: Some(|resolve| {
            format!(
                "Not ready to hand your terminal over? `start-background-server` in the palette (`{}`) serves the web UI behind your back instead. The top bar starts counting who wandered in.",
                resolve(Action::OpenPalette)
            )
        }),
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

    fn rendered_tui_tips() -> Vec<String> {
        let resolve = |a: Action| format!("{a:?}");
        WELCOME_TIPS
            .iter()
            .filter_map(|t| t.tui.map(|f| f(&resolve)))
            .collect()
    }

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

    /// House style: em-dashes read as machine-written prose, so tip text uses
    /// commas or full stops instead. Applies to both surfaces.
    #[test]
    fn no_tip_text_contains_an_em_dash() {
        for (i, web) in web_tips().iter().enumerate() {
            assert!(!web.contains('—'), "web tip {i} contains an em-dash: {web}");
        }
        for (i, tui) in rendered_tui_tips().iter().enumerate() {
            assert!(!tui.contains('—'), "tui tip {i} contains an em-dash: {tui}");
        }
    }

    /// The web has no command palette and no keyboard shortcut for its cog
    /// menu, so a web tip that names a key or a palette is pointing at a
    /// surface the reader is not on. Name the affordance instead: the cog app
    /// menu, a `⋯` menu, a button, the Preferences dialog.
    #[test]
    fn web_tips_never_name_a_key_or_a_palette() {
        for (i, web) in web_tips().iter().enumerate() {
            for banned in ["Ctrl-", "Ctrl+", "palette", "keybind"] {
                assert!(
                    !web.contains(banned),
                    "web tip {i} mentions {banned:?}, which does not exist on the web: {web}"
                );
            }
        }
    }

    /// A regression guard, NOT a target: these floors exist so a refactor
    /// cannot quietly strip the web renderings back off the shared tips. Adding
    /// tips is welcome; dropping below what already shipped is the bug.
    #[test]
    fn the_surfaces_keep_the_tips_they_have() {
        let web = web_tips().len();
        let shared = WELCOME_TIPS
            .iter()
            .filter(|t| t.web.is_some() && t.tui.is_some())
            .count();
        assert!(web >= 30, "web tips fell to {web}");
        assert!(shared >= 30, "shared tips fell to {shared}");
    }
}
