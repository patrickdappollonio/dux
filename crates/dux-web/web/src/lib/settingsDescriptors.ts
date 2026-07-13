// Pure, data-driven model for the Ctrl+K "Settings" modal
// (`CustomizeWebappDialog.tsx`). Each descriptor is one config field the
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
// First-cut subset only (see the architecture plan, section 2): this does
// NOT expose every `[ui]`/`[capabilities]` field, just the ones judged safe,
// portable, and low blast-radius for a first pass. `github_integration`,
// `terminal_identity`, `clipboard_passthrough`, and the numeric infra knobs
// (`agent_scrollback_lines`, `branch_sync_interval`, `pr_poll_interval_seconds`,
// `agent_tabs_max`) are deliberately deferred.

import type { Bootstrap } from "./bootstrapApi"
import { DEFAULT_THEME_NAME } from "./bootstrapApi"

export type SettingSurface = "web" | "both" | "tui"

export type SettingControl =
  | { kind: "bool" }
  | { kind: "number"; min: number; max: number; zeroMeaning?: string; unit?: string }
  | { kind: "enum"; options: { value: string; label: string }[] }
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
   * read/buildWrites/saveSettings path every other row uses. */
  writeTarget: "settings" | "identity" | "changesPane"
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

// Mirrors `dux_core::config::THEME_BUILTIN_NAMES` (`crates/dux-core/src/
// config.rs`), the fixed, documented theme subset the settings-PATCH
// endpoint validates against. Keep this list mirrored with that constant AND
// with the `theme` field's config-template comment in
// `crates/dux-tui/src/config.rs`.
const THEME_BUILTIN_NAMES = [
  "dux_dark",
  "catppuccin_mocha",
  "catppuccin_frappe",
  "nord",
  "dracula",
  "gruvbox_dark",
  "tokyo_night",
  "solarized_dark",
  "one_dark",
  "rose_pine",
]

function themeLabel(name: string): string {
  return name
    .split("_")
    .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
    .join(" ")
}

// Mirrors the server-side clamp ceilings in `crates/dux-core/src/config.rs`
// (`MAX_STATUS_CLEAR_SECONDS`, `MAX_ATTENTION_GRACE_SECONDS`,
// `MAX_DIFF_TAB_WIDTH`). These bound the number inputs for UX only. The
// server re-clamps and is authoritative; the post-save bootstrap refetch
// reflects whatever it actually saved.
const MAX_STATUS_CLEAR_SECONDS = 3_600
const MAX_ATTENTION_GRACE_SECONDS = 300
const MAX_DIFF_TAB_WIDTH = 16

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
          "Shows the right-hand Changes pane (the changed-files list) by default. A runtime toggle from the Changes menu overrides this per session without changing the saved preference.",
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
          "Seconds before a success/info status toast auto-clears. Warning and error toasts are unaffected, they persist until replaced.",
        surface: "both",
        control: {
          kind: "number",
          min: 0,
          max: MAX_STATUS_CLEAR_SECONDS,
          zeroMeaning: "Never auto-clear (sticky, like a warning)",
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
        key: "ui.diff_tab_width",
        label: "Diff tab width",
        description: "How many columns a tab character expands to in the diff viewer.",
        surface: "both",
        control: {
          kind: "number",
          min: 0,
          max: MAX_DIFF_TAB_WIDTH,
          zeroMeaning: "Leave tabs as-is (may render zero-width)",
          unit: "columns",
        },
        default: 4,
        writeTarget: "settings",
        read: (b) => b.diff_tab_width ?? 4,
      },
      {
        key: "ui.show_diff_line_numbers",
        label: "Diff line numbers",
        description: "Shows a line-number gutter in the diff viewer.",
        surface: "both",
        control: { kind: "bool" },
        default: false,
        writeTarget: "settings",
        read: (b) => b.show_diff_line_numbers ?? false,
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
    ],
  },
  {
    surface: "tui",
    caption:
      "Terminal (TUI). Changes the terminal app's config. This won't change this browser. A running dux TUI applies it after its next config reload or restart.",
    settings: [
      {
        key: "ui.theme",
        label: "Theme",
        description:
          "Visual theme for the dux TUI. Built-in options include dux_dark (the default) plus several bundled opaline themes. A custom theme dropped into the config themes directory is only editable from the raw config file.",
        surface: "tui",
        control: {
          kind: "enum",
          options: THEME_BUILTIN_NAMES.map((name) => ({
            value: name,
            label: themeLabel(name),
          })),
        },
        default: DEFAULT_THEME_NAME,
        writeTarget: "settings",
        read: (b) => b.theme ?? DEFAULT_THEME_NAME,
      },
    ],
  },
]

/** Flatten every group's descriptors into a single list, in group order. */
export function allSettingDescriptors(): SettingDescriptor[] {
  return SETTING_GROUPS.flatMap((g) => g.settings)
}
