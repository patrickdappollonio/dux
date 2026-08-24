// Pure, data-driven model for the Preferences modal, opened from the app menu's
// cog (`CustomizeWebappDialog.tsx`). Each descriptor is one config field the
// modal exposes: its label, a human-readable description, which surface(s)
// it affects, its control type, its documented default, and how to read its
// current value out of the `Bootstrap` document. The dialog renders from
// `SETTING_GROUPS` instead of hand-written per-field JSX, so adding/removing
// a field here is the only change needed to change what the modal shows.
//
// KEEP IN SYNC WITH `crates/dux-tui/src/config.rs` (the `config_schema()`
// canonical-template table, see the "KEEP IN SYNC" comment just above that
// function): each `description` below is adapted PROSE from that table's
// `comment` text, not a verbatim copy. See `settingsDescriptors.test.ts` for
// the drift guard (it pins the exposed key SET, not exact description
// strings, so a field's wording can evolve without the test going stale).
//
// A curated subset: this does NOT expose every `[ui]`/`[capabilities]` field,
// just the ones judged safe, portable, and low blast-radius. `terminal_identity`,
// `clipboard_passthrough`, and the numeric infra knobs (`agent_scrollback_lines`,
// `branch_sync_interval`, `pr_poll_interval_seconds`, `agent_tabs_max`) are
// deliberately deferred.
//
// `ui.upload_directory` is deliberately excluded and is not merely deferred:
// it is a PATH, editing one in a free-text row is a poor affordance, and doing
// it properly needs a directory picker this dialog does not have. Its
// companion `ui.upload_write_gitignore` is a plain toggle and IS here.
//
// THIS IS WHERE SETTINGS LIVE. A user preference is a row here, never an app-menu
// item: the menu carries actions and dialogs. The web command palette used to
// carry six preference-shaped toggles, four of which already existed here under a
// second name; the other two (`ui.github_integration`,
// `defaults.enable_randomized_pet_name_by_default`) became rows here when it was
// removed.

import type { Bootstrap } from "./bootstrapApi"
import {
  MAX_TERMINAL_FONT_SIZE,
  MIN_TERMINAL_FONT_SIZE,
} from "./terminalFont"

export type SettingSurface = "web" | "both"

export type SettingControl =
  | { kind: "bool" }
  | { kind: "number"; min: number; max: number; zeroMeaning?: string; unit?: string }
  | { kind: "enum"; options: { value: string; label: string }[] }
  /** Like "enum", but the option list isn't known statically: it is resolved
   * at render time from a live `Bootstrap` field (currently only
   * `available_providers`), so the client can never offer a provider name the
   * server doesn't have configured. See `CustomizeWebappDialog.tsx`'s
   * `SettingControl` for the resolution. */
  | { kind: "enum-dynamic"; source: "available_providers" }
  | { kind: "favicon" }
  | { kind: "text"; maxLen: number }

export type SettingValue = boolean | number | string

export interface SettingDescriptor {
  /** Dotted config key, e.g. "ui.status_clear_seconds" or "server.title". */
  key: string
  /** Short human label shown as the row's primary text. */
  label: string
  /** One or two sentences of prose, shown muted under the label. */
  description: string
  surface: SettingSurface
  control: SettingControl
  default: SettingValue
  /** Which write path Save uses for this row: the generic settings PATCH, the
   * dedicated instance-identity endpoint (title/favicon only, see the
   * CLAUDE.md web-UI tenet: "keep title/favicon on the existing endpoint"), or
   * the bespoke Changes-pane visibility endpoint. `"changesPane"` is bespoke
   * because the store tracks an optimistic `changesPaneOverride` for that one
   * field (the Changes menu toggles it live outside this dialog too), so its
   * row is wired directly to `changesPaneVisible()`/`setChangesPaneVisibility`
   * in `CustomizeWebappDialog.tsx` rather than through the generic
   * read/buildWrites/saveSettings path every other row uses.
   *
   * `"github"` is bespoke for a different reason: flipping `ui.github_integration`
   * has SIDE EFFECTS beyond the config write (it arms or disarms the background
   * PR-sync poll, kicks an initial refresh, and clears cached PR statuses), and
   * that logic lives behind `POST /api/v1/ui/toggle-github-integration`. Routing
   * the row there reuses it instead of forking it into the generic settings PATCH.
   *
   * HAZARD, and the reason this is safe: that endpoint is a blind read-and-FLIP,
   * while this modal saves EXPLICIT values. The two only agree because
   * `buildWrites` diffs each row against its pre-touch baseline and emits it ONLY
   * when it actually changed, so "present in the write" implies "flip". If anyone
   * ever "simplifies" `persist` to write unconditionally, this silently INVERTS
   * the setting. Pinned by `CustomizeWebappDialog.test.tsx`'s "does not call the
   * GitHub endpoint when the row is unchanged".
   *
   * `"tailscale"` is bespoke for a third reason: saving `[server] tailscale` is
   * only half of what the row does. The other half moves the RUNNING listener
   * (stop or start the interface watcher, bind or drop the Tailscale leg, move
   * the Host guard's tailnet-literal rule with it), which only the serve loop
   * can perform, and the endpoint answers with what it actually did. Unlike
   * `"github"` this one carries an explicit value, so the unchanged-row skip is
   * an optimization here rather than a correctness requirement. */
  writeTarget: "settings" | "identity" | "changesPane" | "github" | "tailscale"
  /** True when the config field is the NEGATIVE of what this row shows: the row
   * says "Show the welcome screen" while `ui.disable_automated_welcome_screen`
   * says the opposite. Every row in this modal is phrased positively, because a
   * "Disable X" toggle turned off is a double negative the reader has to unpick.
   *
   * The contract, and it is only two places: `read` returns the value AS SHOWN
   * (already flipped), and `buildWrites` in `CustomizeWebappDialog.tsx` flips it
   * back once, immediately before it goes on the wire. Nothing else in the
   * pipeline knows or cares, so the unchanged-row skip still compares
   * shown-value to shown-value. Bool rows only. */
  inverted?: boolean
  /** A lock this RUN of the server puts on the row: the sentence saying why the
   * value cannot take effect until the next run, or `null` when it can. A locked
   * row renders disabled and shows this sentence instead of `description`, so
   * the dialog never offers a write the server is going to refuse. Absent on
   * every row that no run-scoped flag can override. */
  lockedBy?: (b: Bootstrap) => string | null
  /** Reads the current value out of the live Bootstrap document, falling back
   * to `default` when an older server omits the field. NOTE: for the
   * `"changesPane"`-targeted row this is NOT the effective value shown in the
   * dialog, the override-aware `changesPaneVisible()` in `store.ts` is. It is
   * kept here only so generic helpers (the drift-guard test, `defaultLabel`)
   * that read every descriptor still have something to call. */
  read: (b: Bootstrap) => SettingValue
}

export interface SettingGroup {
  surface: SettingSurface
  /** Shown once above the group's rows (CLAUDE.md web-UI tenet: group by
   * surface with an explicit per-group caption rather than repeating the
   * caveat per row). */
  caption: string
  settings: SettingDescriptor[]
}

// Mirrors the server-side clamp ceilings in `crates/dux-core/src/config.rs`
// (`MAX_STATUS_CLEAR_SECONDS`, `MAX_ATTENTION_GRACE_SECONDS`). These bound
// the number inputs for UX only. The server re-clamps and is authoritative;
// the post-save bootstrap refetch reflects whatever it actually saved.
const MAX_STATUS_CLEAR_SECONDS = 3_600
const MAX_ATTENTION_GRACE_SECONDS = 300
// Mirrors `MAX_UPLOAD_PASTED_TEXT_CHARS` / `DEFAULT_UPLOAD_PASTED_TEXT_CHARS`
// in `crates/dux-core/src/config.rs`. The floor is deliberately NOT mirrored as
// the input's `min`: `0` is a real value (switch the behaviour off) and the
// server clamps anything between 1 and the floor up with a warning, so bounding
// the input at the floor would make the off switch unreachable from here.
const MAX_UPLOAD_PASTED_TEXT_CHARS = 100_000
const MIN_UPLOAD_PASTED_TEXT_CHARS = 200
const DEFAULT_UPLOAD_PASTED_TEXT_CHARS = 1_000
// MIN_TERMINAL_FONT_SIZE/MAX_TERMINAL_FONT_SIZE are imported above from
// terminalFont.ts rather than redeclared here (that file mirrors the
// server-side bounds in `crates/dux-core/src/config.rs`). UX bounds only; the
// server re-clamps.

export const SETTING_GROUPS: SettingGroup[] = [
  {
    surface: "web",
    caption: "This browser (Web). These affect the web UI you're looking at.",
    settings: [
      {
        key: "server.title",
        label: "Instance name",
        description:
          "The display name for this dux instance. Shown as the browser tab title and the sidebar wordmark. Set a distinct value per instance (e.g. \"dux #1\" or \"dux (prod)\") to tell several dux tabs apart at a glance.",
        surface: "web",
        control: { kind: "text", maxLen: 200 },
        default: "dux",
        writeTarget: "identity",
        read: (b) => b.title ?? "dux",
      },
      {
        key: "server.favicon",
        label: "Favicon",
        description:
          "A tint color for the browser tab favicon, so several dux tabs are easy to tell apart at a glance.",
        surface: "web",
        control: { kind: "favicon" },
        default: "",
        writeTarget: "identity",
        read: (b) => b.favicon ?? "",
      },
      {
        key: "ui.show_changes_pane",
        label: "Show the Changes pane",
        description:
          "Shows the right-hand Changes pane (the changed-files list). Hiding it from the Changes menu or showing it from the header button saves this same preference, so this row and those controls always agree.",
        surface: "web",
        control: { kind: "bool" },
        default: true,
        writeTarget: "changesPane",
        read: (b) => b.show_changes_pane ?? true,
      },
      {
        key: "ui.copy_on_select",
        label: "Copy on select",
        description:
          "Selecting text in the web terminal automatically copies it to the clipboard (X11-style highlight-to-copy).",
        surface: "web",
        control: { kind: "bool" },
        default: true,
        writeTarget: "settings",
        read: (b) => b.copy_on_select ?? true,
      },
      {
        key: "ui.terminal_font_family",
        label: "Terminal font",
        description:
          "Name a font installed on THIS device (e.g. \"Fira Code\" or \"Cascadia Code\") to use in the web terminal. It is placed ahead of dux's bundled terminal font, so the bundled font still fills in any glyph (box drawing, blocks, braille, arrows) your chosen font lacks. Leave blank to use only the bundled font.",
        surface: "web",
        control: { kind: "text", maxLen: 200 },
        default: "",
        writeTarget: "settings",
        read: (b) => b.terminal_font_family ?? "",
      },
      {
        key: "ui.terminal_font_size",
        label: "Terminal font size",
        description: "The web terminal's font size, in pixels.",
        surface: "web",
        control: {
          kind: "number",
          min: MIN_TERMINAL_FONT_SIZE,
          max: MAX_TERMINAL_FONT_SIZE,
          unit: "px",
        },
        default: 14,
        writeTarget: "settings",
        read: (b) => b.terminal_font_size ?? 14,
      },
      {
        key: "ui.compose_bar",
        label: "Touch compose bar",
        description:
          "On a touch device, adds a compose box below the terminal keys: type with your keyboard's autocorrect and swipe input, then Send delivers the message and presses Enter. Without it, tapping the terminal types directly into it. Automatic follows your browser's report of whether you point with a finger, which (unlike a screen-size rule) does not change when you rotate a tablet. It cannot tell a tablet with a keyboard case from one without, so choose Always or Never if your device is one dux guesses wrong.",
        surface: "web",
        control: {
          kind: "enum",
          options: [
            { value: "auto", label: "Automatic" },
            { value: "always", label: "Always" },
            { value: "never", label: "Never" },
          ],
        },
        default: "auto",
        writeTarget: "settings",
        read: (b) => b.compose_bar ?? "auto",
      },
      {
        key: "ui.mobile_top_bar",
        // "Mobile" is accurate here and deliberately kept: the top bar is the
        // phone shell's own chrome (MobileShell renders it and nothing else
        // does), so unlike the keys below it genuinely does not exist in the
        // wide layout.
        label: "Mobile terminal top bar",
        description:
          "On phones, shows the terminal screen's top bar: the back chevron, branch crumb and actions, plus the agent tab strip. Hide it to give those rows to the terminal; bring it back from the input ⋯ menu below the terminal or from this Preferences dialog.",
        surface: "web",
        control: { kind: "bool" },
        default: true,
        writeTarget: "settings",
        read: (b) => b.mobile_top_bar ?? true,
      },
      {
        key: "ui.mobile_accessory_bar",
        // The key stays `mobile_accessory_bar` for compatibility, but the copy
        // says TOUCH: the keys travel with the pointer, so a tablet in
        // landscape gets them inside the desktop layout, and this preference
        // is shared across your devices. Naming it "phones" sent a user
        // looking for a bar that was never the phone's alone.
        label: "Touch terminal keys",
        description:
          "On a touch device, shows the terminal-keys bar (Esc, Tab, Ctrl, Alt and the arrows) above the compose box, in the wide layout as well as on a phone. Hide it to give those rows to the terminal; bring it back from the input ⋯ menu below the terminal or from this Preferences dialog.",
        surface: "web",
        control: { kind: "bool" },
        default: true,
        writeTarget: "settings",
        read: (b) => b.mobile_accessory_bar ?? true,
      },
      {
        key: "ui.upload_write_gitignore",
        label: "Hide dropped and pasted files from git",
        description:
          "Files you drop or paste onto an agent are saved inside its worktree, so git would otherwise show them as untracked changes. This keeps a .gitignore in that upload folder so they stay invisible. Turn it off if you mean to commit what you hand the agent. An existing .gitignore is never touched.",
        surface: "web",
        control: { kind: "bool" },
        default: true,
        writeTarget: "settings",
        read: (b) => b.upload_write_gitignore ?? true,
      },
      {
        key: "ui.upload_pasted_text_chars",
        label: "Save long pastes as a file",
        description:
          `Paste more than this many characters into an agent and dux saves the text as a .txt file in the upload folder and pastes that file's path instead, into the message box on a phone or straight at the prompt otherwise. An agent's context window is finite, but it can read a document when it needs to, so a path costs it almost nothing while a wall of text costs the window either way. Press Ctrl+Shift+v (Cmd+Shift+v on a Mac) to paste text as text just this once. Never applies to a terminal, where a long paste is usually a command. Anything between 1 and ${MIN_UPLOAD_PASTED_TEXT_CHARS} is raised to ${MIN_UPLOAD_PASTED_TEXT_CHARS}; use 0 to switch it off.`,
        surface: "web",
        control: {
          kind: "number",
          min: 0,
          max: MAX_UPLOAD_PASTED_TEXT_CHARS,
          zeroMeaning: "Never; always paste text as text",
          unit: "characters",
        },
        default: DEFAULT_UPLOAD_PASTED_TEXT_CHARS,
        writeTarget: "settings",
        // ABSENT MEANS OFF, not "means the default". An older server publishes
        // nothing here, `TerminalPane` reads that as 0 and files nothing away,
        // and `bootstrapApi.ts` documents the rule. Reading it as 1000 in this
        // one place showed a threshold that was not in force, and a user who
        // saved the dialog would have switched the feature on without asking
        // for it. `default` below is the shipped value, which is a different
        // question and is answered separately.
        read: (b) => b.upload_pasted_text_chars ?? 0,
      },
      {
        key: "capabilities.web_notifications",
        label: "Desktop notifications",
        description:
          "Bridges an agent's notification sequences to a browser desktop notification while this tab is backgrounded. Still gated on the browser's own notification permission.",
        surface: "web",
        control: { kind: "bool" },
        default: true,
        writeTarget: "settings",
        read: (b) => b.web_notifications ?? true,
      },
    ],
  },
  {
    surface: "both",
    caption: "Both surfaces. Affects the web UI and the terminal app (TUI).",
    settings: [
      {
        key: "ui.status_clear_seconds",
        label: "Status message auto-clear",
        description:
          "Seconds before a success/info status toast auto-clears. Warnings stay up twice as long and errors four times as long, so this one number moves all of them. Set it to 0 to keep status toasts on screen until you dismiss them.",
        surface: "both",
        control: {
          kind: "number",
          min: 0,
          max: MAX_STATUS_CLEAR_SECONDS,
          // Not "like a warning" any more: a warning now retires at twice this
          // window rather than persisting, so the old analogy taught the
          // opposite of the truth. "Sticky" is also a specific thing now (the
          // handful of messages that wait for the user whatever this is set to),
          // so it cannot double as a loose description of this setting.
          zeroMeaning: "Never auto-clear (stays until you dismiss it)",
          unit: "seconds",
        },
        default: 6,
        writeTarget: "settings",
        read: (b) => b.status_clear_seconds ?? 6,
      },
      {
        key: "ui.attention_grace_seconds",
        label: "Attention grace",
        description:
          "Seconds the attention indicators stay visible after you return to dux (the browser tab regains focus, or the TUI's terminal window regains focus), before the focused agent's needs-attention flag clears.",
        surface: "both",
        control: {
          kind: "number",
          min: 0,
          max: MAX_ATTENTION_GRACE_SECONDS,
          zeroMeaning: "Clear the indicator immediately",
          unit: "seconds",
        },
        default: 3,
        writeTarget: "settings",
        read: (b) => b.attention_grace_seconds ?? 3,
      },
      {
        key: "ui.attention_indicator",
        label: "Show attention indicator",
        description:
          "Shows an indicator when an agent asks for attention (a permission prompt, a finished turn). When off, no attention cue is shown on either surface.",
        surface: "both",
        control: { kind: "bool" },
        default: true,
        writeTarget: "settings",
        read: (b) => b.attention_indicator ?? true,
      },
      {
        key: "ui.attention_on_bell",
        label: "Attention on terminal bell",
        description:
          "Also treats a plain terminal bell as an attention request. Has no effect when \"Show attention indicator\" is off.",
        surface: "both",
        control: { kind: "bool" },
        default: true,
        writeTarget: "settings",
        read: (b) => b.attention_on_bell ?? true,
      },
      {
        key: "ui.auto_reopen_agents",
        label: "Reopen agents on startup",
        description:
          "When dux starts, agents that were running when it last exited and have auto-reopen enabled are relaunched automatically. Per-project and per-agent switches can opt out.",
        surface: "both",
        control: { kind: "bool" },
        default: false,
        writeTarget: "settings",
        read: (b) => b.auto_reopen_agents ?? false,
      },
      {
        key: "ui.always_show_tab_strip",
        label: "Always show tab strip",
        description:
          "Always shows the agent tab strip, even when a session has only one tab. Off shows the strip only once a session has two or more tabs.",
        surface: "both",
        control: { kind: "bool" },
        default: false,
        writeTarget: "settings",
        read: (b) => b.always_show_tab_strip ?? false,
      },
      {
        key: "ui.pr_banner_position",
        label: "PR banner position",
        description:
          "Where the pull-request status banner sits relative to the agent's terminal.",
        surface: "both",
        control: {
          kind: "enum",
          options: [
            { value: "top", label: "Top" },
            { value: "bottom", label: "Bottom" },
          ],
        },
        default: "bottom",
        writeTarget: "settings",
        read: (b) => b.pr_banner_position ?? "bottom",
      },
      {
        key: "capabilities.hyperlinks",
        label: "Clickable hyperlinks",
        description: "Renders OSC 8 hyperlinks an agent prints as clickable (http/https only).",
        surface: "both",
        control: { kind: "bool" },
        default: true,
        writeTarget: "settings",
        read: (b) => b.hyperlinks ?? true,
      },
      {
        key: "ui.github_integration",
        label: "GitHub integration",
        description:
          "Syncs pull-request status for your agents in the background using the `gh` CLI, showing a PR pill on branches with an open, merged, or closed pull request. When off, dux stops polling and clears cached PR statuses. Requires `gh` to be installed and authenticated.",
        surface: "both",
        control: { kind: "bool" },
        default: true,
        // NOT "settings": see the writeTarget doc above. Flipping this drives
        // engine-side PR-sync side effects that only the dedicated endpoint has.
        writeTarget: "github",
        read: (b) => b.github_integration ?? true,
      },
      {
        key: "server.tailscale",
        label: "Bind your Tailscale address",
        description:
          "Whether dux also serves on this machine's Tailscale address. \"Auto\" binds it whenever the interface appears and drops it when it goes, so a laptop that roams keeps working. \"Yes\" looks once and keeps whatever it finds. \"No\" never binds it. Changing this applies to the listener that is serving right now, so choosing \"No\" from a browser on your tailnet will close this tab's connection; reopen dux on its other address.",
        surface: "both",
        control: {
          kind: "enum",
          options: [
            { value: "auto", label: "Auto" },
            { value: "yes", label: "Yes" },
            { value: "no", label: "No" },
          ],
        },
        default: "auto",
        // NOT "settings": see the writeTarget doc above. Saving the value is
        // only half of it; the other half moves a live listener.
        writeTarget: "tailscale",
        // `--no-tailscale` wins over the config for as long as the run lasts,
        // so the listener would refuse every value but "no".
        lockedBy: (b) =>
          b.tailscale_forced_no
            ? "This run of dux was started with `--no-tailscale`, so the Tailscale address stays unbound however this is set. Your choice is saved and used the next time dux starts without that flag."
            : null,
        read: (b) => b.tailscale_mode ?? "auto",
      },
      {
        key: "defaults.enable_randomized_pet_name_by_default",
        label: "Random pet-name default for new agents",
        description:
          "New agent prompts start with a random pet name already filled in. The new-agent dialog still has its own per-open randomize checkbox, seeded from this default.",
        surface: "both",
        control: { kind: "bool" },
        default: false,
        writeTarget: "settings",
        // The bootstrap projects this as `randomize_agent_names_by_default`,
        // not under its config key's name.
        read: (b) => b.randomize_agent_names_by_default ?? false,
      },
      {
        key: "ui.disable_automated_welcome_screen",
        label: "Show the welcome screen on a new install",
        description:
          "Shows a one-time welcome screen the first time dux runs, explaining projects, agents, and worktrees. Turning this off skips it automatically; the app menu's \"Welcome screen…\" still opens it any time.",
        surface: "both",
        control: { kind: "bool" },
        // Presented POSITIVELY ("show it") while the config field is a
        // NEGATIVE ("disable it"), because a row that reads "Disable X" turns
        // every toggle into a double negative. The dialog inverts on read and on
        // write: `buildWrites` in `CustomizeWebappDialog.tsx` flips it exactly once.
        inverted: true,
        default: true,
        writeTarget: "settings",
        read: (b) => !(b.disable_automated_welcome_screen ?? false),
      },
      {
        key: "ui.disable_release_notes",
        label: "Show what's new after an update",
        description:
          "After dux updates to a new version, shows that release's highlights once, fetched from GitHub. Turning this off skips it automatically; the app menu's \"What's new…\" still opens it any time.",
        surface: "both",
        control: { kind: "bool" },
        inverted: true,
        default: true,
        writeTarget: "settings",
        read: (b) => !(b.disable_release_notes ?? false),
      },
      {
        key: "defaults.provider",
        label: "Default provider for new agents",
        description:
          "The global default provider used for new agents in projects that don't set their own project-specific override. A project's own default provider (set in that project's settings) always wins over this one.",
        surface: "both",
        control: { kind: "enum-dynamic", source: "available_providers" },
        default: "claude",
        writeTarget: "settings",
        // The bootstrap projects this as `global_default_provider` (not
        // `default_provider`), to keep it unambiguous next to the
        // per-project `default_provider` field the project settings dialog
        // reads.
        read: (b) => b.global_default_provider ?? "claude",
      },
    ],
  },
]

/** Flatten every group's descriptors into a single list, in group order. */
export function allSettingDescriptors(): SettingDescriptor[] {
  return SETTING_GROUPS.flatMap((g) => g.settings)
}
