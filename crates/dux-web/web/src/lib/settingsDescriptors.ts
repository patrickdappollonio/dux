// Data model for the Preferences modal (`CustomizeWebappDialog.tsx`), which
// renders from `SETTING_GROUPS` rather than per-field JSX. The rules this list
// respects:
//
// - Each `description` is prose adapted from the `config_schema()` table in
//   `crates/dux-tui/src/config.rs`; keep the two in step.
// - The exposed set is curated, not exhaustive: a field belongs here only when
//   it is safe, portable, and low blast-radius. `settingsDescriptors.test.ts`
//   pins the exposed key set, not the description strings.
// - `ui.upload_directory` stays out: editing a path in a free-text row needs a
//   directory picker this dialog does not have.
// - A user preference is a row here, never an app-menu item; the menu carries
//   actions and dialogs.

import type { Bootstrap } from "./bootstrapApi"
import {
  MAX_TERMINAL_FONT_SIZE,
  MIN_TERMINAL_FONT_SIZE,
} from "./terminalFont"

export type SettingSurface = "web" | "both" | "tui"

export type SettingControl =
  | { kind: "bool" }
  | { kind: "number"; min: number; max: number; zeroMeaning?: string; unit?: string }
  | { kind: "enum"; options: { value: string; label: string }[] }
  /** Like "enum", but the options resolve at render time from a live `Bootstrap`
   * field, so the client cannot offer a provider the server has not configured. */
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
  /** Which write path Save uses for this row. Three targets are bespoke because
   * their fields are not plain config writes:
   * - `"changesPane"`: the store keeps an optimistic override for it (the
   *   Changes menu toggles it live too), so the row is wired to
   *   `changesPaneVisible()`/`setChangesPaneVisibility`.
   * - `"github"`: flipping it arms or disarms the PR-sync poll and clears
   *   cached statuses, which lives behind the dedicated endpoint. That endpoint
   *   is a blind read-and-flip, so `buildWrites` must emit the row only when it
   *   changed; writing unconditionally inverts the setting.
   * - `"tailscale"`: the endpoint also moves the running listener, which only
   *   the serve loop can do. It carries an explicit value, so skipping an
   *   unchanged row is an optimization here rather than correctness. */
  writeTarget: "settings" | "identity" | "changesPane" | "github" | "tailscale"
  /** True when the config field is the negative of what this row shows, so that
   * every row can be phrased positively. `read` returns the value as shown and
   * `buildWrites` flips it back once, immediately before the wire, so the
   * unchanged-row skip compares shown value to shown value. Bool rows only. */
  inverted?: boolean
  /** Why this run of the server cannot honour the value until it restarts, or
   * `null` when it can. A locked row renders disabled and shows this sentence
   * instead of `description`. */
  lockedBy?: (b: Bootstrap) => string | null
  /** Reads the current value out of the live Bootstrap document, falling back
   * to `default` when an older server omits the field. For the `"changesPane"`
   * row this is not the effective value the dialog shows; the override-aware
   * `changesPaneVisible()` in `store.ts` is. */
  read: (b: Bootstrap) => SettingValue
}

export interface SettingGroup {
  surface: SettingSurface
  /** Shown once above the group's rows, so the surface caveat is stated per
   * group rather than repeated on every row. */
  caption: string
  settings: SettingDescriptor[]
}

// Mirror the server-side clamp ceilings in `crates/dux-core/src/config.rs`;
// they bound the number inputs for UX only and the server re-clamps.
const MAX_STATUS_CLEAR_SECONDS = 3_600
const MAX_ATTENTION_GRACE_SECONDS = 300
// The floor is deliberately not the input's `min`: 0 switches the behaviour
// off, and the server raises anything between 1 and the floor.
const MAX_UPLOAD_PASTED_TEXT_CHARS = 100_000
const MIN_UPLOAD_PASTED_TEXT_CHARS = 200
const DEFAULT_UPLOAD_PASTED_TEXT_CHARS = 4_000

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
        key: "ui.mobile_accessory_bar",
        // The key stays `mobile_accessory_bar` for config compatibility while
        // the copy says touch: the keys follow the pointer, not the layout.
        label: "Touch terminal keys",
        description:
          "On a touch device, shows the terminal-keys bar (Esc, Tab, Ctrl, Alt and the arrows) above the compose box, in the wide layout as well as on a phone. Hide it to give those rows to the terminal; bring it back from the input ⋯ menu beside the message box, from the terminal's own ⋯ menu when there is no typing bar left to hold one, or from this Preferences dialog.",
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
        // Absent means off rather than the shipped default: an older server
        // publishes nothing here and `TerminalPane` reads that as 0.
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
          "Seconds before a success/info status toast auto-clears. Warnings stay up three times as long and errors four times as long, so this one number moves all of them. Set it to 0 to keep status toasts on screen until you dismiss them.",
        surface: "both",
        control: {
          kind: "number",
          min: 0,
          max: MAX_STATUS_CLEAR_SECONDS,
          // Neither "like a warning" nor "sticky" names this: a warning retires
          // at three times this window, and sticky messages ignore it entirely.
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
        // The bootstrap projects this under a different name from its config key.
        read: (b) => b.randomize_agent_names_by_default ?? false,
      },
      {
        key: "ui.disable_automated_welcome_screen",
        label: "Show the welcome screen on a new install",
        description:
          "Shows a one-time welcome screen the first time dux runs, explaining projects, agents, and worktrees. Turning this off skips it automatically; the app menu's \"Welcome screen…\" still opens it any time.",
        surface: "both",
        control: { kind: "bool" },
        // Shown as "show it" while the config field says "disable it".
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
        // Named `global_default_provider` on the bootstrap to stay unambiguous
        // next to the per-project `default_provider` field.
        read: (b) => b.global_default_provider ?? "claude",
      },
    ],
  },
  {
    surface: "tui",
    caption: "Terminal UI. These affect the terminal app only.",
    settings: [
      {
        key: "ui.tab_reaches_agent",
        label: "Send Tab to the agent",
        description:
          "In the terminal app, Tab and Shift-Tab are typed into the agent in the center pane instead of moving between panes. Off by default, because Tab has moved between panes since dux's first version. The pane chords (Ctrl-o and Ctrl-y unless you have rebound them) move between panes either way. The web terminal is unaffected: Tab always reaches the agent there.",
        surface: "tui",
        control: { kind: "bool" },
        default: false,
        writeTarget: "settings",
        read: (b) => b.tab_reaches_agent ?? false,
      },
    ],
  },
]

/** Flatten every group's descriptors into a single list, in group order. */
export function allSettingDescriptors(): SettingDescriptor[] {
  return SETTING_GROUPS.flatMap((g) => g.settings)
}
