import { describe, expect, it } from "vitest"

import type { Bootstrap } from "./bootstrapApi"
import { DEFAULT_THEME_NAME } from "./bootstrapApi"
import { SETTING_GROUPS, allSettingDescriptors } from "./settingsDescriptors"

// A fully-populated bootstrap so every descriptor's `read()` has a real,
// distinguishable value to check against (never the fallback default).
const sampleBootstrap: Bootstrap = {
  available_providers: [],
  macros: [],
  palette_commands: [],
  welcome_tips: [],
  dux_version: "v0.0.0",
  randomize_agent_names_by_default: false,
  gh_available: false,
  github_integration: false,
  copy_on_select: false,
  attention_grace_seconds: 11,
  web_notifications: false,
  hyperlinks: false,
  clipboard_passthrough: "focused",
  pr_banner_position: "top",
  agent_scrollback_lines: 10000,
  show_changes_pane: false,
  global_env: {},
  status_clear_seconds: 42,
  title: "prod dux",
  favicon: "amber",
  agent_tabs_max: 20,
  always_show_tab_strip: true,
  attention_indicator: false,
  attention_on_bell: false,
  diff_tab_width: 8,
  show_diff_line_numbers: true,
  theme: "nord",
}

describe("settingsDescriptors", () => {
  it("groups are ordered web, both, tui", () => {
    expect(SETTING_GROUPS.map((g) => g.surface)).toEqual(["web", "both", "tui"])
  })

  // NOTE: this only catches an accidental key change WITHIN this file (a typo
  // in a descriptor's `key`, or an accidental add/remove). It is NOT a
  // cross-language guard against the Rust config schema, the hand-typed `Set`
  // below lives in this same file, so it can drift right alongside
  // `SETTING_GROUPS` without failing. The real cross-language guard is the
  // "KEEP IN SYNC WITH" comment on `config_schema()` in
  // `crates/dux-tui/src/config.rs`, which is a human-reviewed pointer, not an
  // automated check. A full build-time cross-check against the Rust schema is
  // out of scope for this test.
  it("exposes exactly the first-cut subset of keys (catches accidental key changes within this file)", () => {
    const keys = allSettingDescriptors().map((d) => d.key)
    expect(new Set(keys)).toEqual(
      new Set([
        "server.title",
        "server.favicon",
        "ui.show_changes_pane",
        "ui.copy_on_select",
        "capabilities.web_notifications",
        "ui.status_clear_seconds",
        "ui.attention_grace_seconds",
        "ui.attention_indicator",
        "ui.attention_on_bell",
        "ui.always_show_tab_strip",
        "ui.pr_banner_position",
        "ui.diff_tab_width",
        "ui.show_diff_line_numbers",
        "capabilities.hyperlinks",
        "ui.theme",
      ]),
    )
    // No duplicate keys across groups.
    expect(keys.length).toBe(new Set(keys).size)
  })

  it("every descriptor has a non-empty label and description", () => {
    for (const d of allSettingDescriptors()) {
      expect(d.label.trim().length, `label for ${d.key}`).toBeGreaterThan(0)
      expect(
        d.description.trim().length,
        `description for ${d.key}`,
      ).toBeGreaterThan(0)
    }
  })

  it("read() returns the bootstrap value for each descriptor", () => {
    for (const d of allSettingDescriptors()) {
      const value = d.read(sampleBootstrap)
      expect(value, `read() for ${d.key}`).not.toBeUndefined()
    }
    const byKey = Object.fromEntries(
      allSettingDescriptors().map((d) => [d.key, d.read(sampleBootstrap)]),
    )
    expect(byKey["server.title"]).toBe("prod dux")
    expect(byKey["server.favicon"]).toBe("amber")
    expect(byKey["ui.show_changes_pane"]).toBe(false)
    expect(byKey["ui.copy_on_select"]).toBe(false)
    expect(byKey["capabilities.web_notifications"]).toBe(false)
    expect(byKey["ui.status_clear_seconds"]).toBe(42)
    expect(byKey["ui.attention_grace_seconds"]).toBe(11)
    expect(byKey["ui.attention_indicator"]).toBe(false)
    expect(byKey["ui.attention_on_bell"]).toBe(false)
    expect(byKey["ui.always_show_tab_strip"]).toBe(true)
    expect(byKey["ui.pr_banner_position"]).toBe("top")
    expect(byKey["ui.diff_tab_width"]).toBe(8)
    expect(byKey["ui.show_diff_line_numbers"]).toBe(true)
    expect(byKey["capabilities.hyperlinks"]).toBe(false)
    expect(byKey["ui.theme"]).toBe("nord")
  })

  it("read() falls back to the documented default on an older bootstrap missing the field", () => {
    const bare = { ...sampleBootstrap } as Partial<Bootstrap>
    delete bare.attention_indicator
    delete bare.attention_on_bell
    delete bare.diff_tab_width
    delete bare.show_diff_line_numbers
    delete bare.theme
    const byKey = Object.fromEntries(
      allSettingDescriptors().map((d) => [d.key, d.read(bare as Bootstrap)]),
    )
    expect(byKey["ui.attention_indicator"]).toBe(true)
    expect(byKey["ui.attention_on_bell"]).toBe(true)
    expect(byKey["ui.diff_tab_width"]).toBe(4)
    expect(byKey["ui.show_diff_line_numbers"]).toBe(false)
    expect(byKey["ui.theme"]).toBe(DEFAULT_THEME_NAME)
  })

  it("number controls declare a zeroMeaning where the config documents one", () => {
    const byKey = Object.fromEntries(allSettingDescriptors().map((d) => [d.key, d]))
    for (const key of [
      "ui.status_clear_seconds",
      "ui.attention_grace_seconds",
      "ui.diff_tab_width",
    ]) {
      const d = byKey[key]
      expect(d.control.kind, key).toBe("number")
      if (d.control.kind === "number") {
        expect(d.control.zeroMeaning, `zeroMeaning for ${key}`).toBeTruthy()
      }
    }
  })

  it("enum controls list at least two options", () => {
    for (const d of allSettingDescriptors()) {
      if (d.control.kind === "enum") {
        expect(d.control.options.length, d.key).toBeGreaterThanOrEqual(2)
      }
    }
  })

  it("the theme select includes the bundled default", () => {
    const theme = allSettingDescriptors().find((d) => d.key === "ui.theme")
    expect(theme?.control.kind).toBe("enum")
    if (theme?.control.kind === "enum") {
      expect(theme.control.options.map((o) => o.value)).toContain(
        DEFAULT_THEME_NAME,
      )
    }
  })
})
