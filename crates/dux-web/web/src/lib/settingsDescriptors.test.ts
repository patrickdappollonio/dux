import { describe, expect, it } from "vitest"

import type { Bootstrap } from "./bootstrapApi"
import { SETTING_GROUPS, allSettingDescriptors } from "./settingsDescriptors"

// A fully-populated bootstrap so every descriptor's `read()` has a real,
// distinguishable value to check against (never the fallback default).
const sampleBootstrap: Bootstrap = {
  available_providers: ["claude", "codex"],
  macros: [],
  welcome_tips: [],
  dux_version: "v0.0.0",
  randomize_agent_names_by_default: false,
  gh_available: false,
  github_integration: false,
  copy_on_select: false,
  terminal_font_family: "Fira Code",
  terminal_font_size: 18,
  compose_bar: "never",
  mobile_top_bar: false,
  mobile_accessory_bar: false,
  upload_write_gitignore: false,
  auto_reopen_agents: true,
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
  global_default_provider: "codex",
  disable_automated_welcome_screen: true,
  disable_release_notes: true,
}

describe("settingsDescriptors", () => {
  it("groups are ordered web, both", () => {
    expect(SETTING_GROUPS.map((g) => g.surface)).toEqual(["web", "both"])
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
        "ui.terminal_font_family",
        "ui.terminal_font_size",
        "ui.compose_bar",
        "ui.mobile_top_bar",
        "ui.mobile_accessory_bar",
        "ui.upload_write_gitignore",
        "ui.upload_pasted_text_chars",
        "ui.auto_reopen_agents",
        "capabilities.web_notifications",
        "ui.status_clear_seconds",
        "ui.attention_grace_seconds",
        "ui.attention_indicator",
        "ui.attention_on_bell",
        "ui.always_show_tab_strip",
        "ui.pr_banner_position",
        "capabilities.hyperlinks",
        "ui.github_integration",
        "ui.disable_automated_welcome_screen",
        "ui.disable_release_notes",
        "defaults.enable_randomized_pet_name_by_default",
        "defaults.provider",
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

  it("the Changes-pane description says the runtime toggle saves this same preference", () => {
    // The runtime toggle (the pane's hide item, the header's show button) goes
    // through setChangesPaneVisibility -> PUT /api/v1/ui/changes-pane, which
    // persists ui.show_changes_pane on every flip. An earlier copy claimed the
    // toggle worked "without changing the saved preference", which was false:
    // the client-side override is only an optimistic echo, cleared once the
    // broadcast config value matches.
    const d = allSettingDescriptors().find(
      (x) => x.key === "ui.show_changes_pane",
    )
    expect(d).toBeDefined()
    const description = d!.description.toLowerCase()
    expect(description).not.toContain("without changing the saved preference")
    expect(description).toContain("same preference")
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
    expect(byKey["ui.terminal_font_family"]).toBe("Fira Code")
    expect(byKey["ui.terminal_font_size"]).toBe(18)
    expect(byKey["ui.compose_bar"]).toBe("never")
    expect(byKey["ui.mobile_top_bar"]).toBe(false)
    expect(byKey["ui.mobile_accessory_bar"]).toBe(false)
    expect(byKey["ui.auto_reopen_agents"]).toBe(true)
    expect(byKey["capabilities.web_notifications"]).toBe(false)
    expect(byKey["ui.status_clear_seconds"]).toBe(42)
    expect(byKey["ui.attention_grace_seconds"]).toBe(11)
    expect(byKey["ui.attention_indicator"]).toBe(false)
    expect(byKey["ui.attention_on_bell"]).toBe(false)
    expect(byKey["ui.always_show_tab_strip"]).toBe(true)
    expect(byKey["ui.pr_banner_position"]).toBe("top")
    expect(byKey["capabilities.hyperlinks"]).toBe(false)
    expect(byKey["ui.github_integration"]).toBe(false)
    expect(byKey["defaults.enable_randomized_pet_name_by_default"]).toBe(false)
    expect(byKey["defaults.provider"]).toBe("codex")
    // The two first-load rows are INVERTED: the bootstrap says "disabled: true",
    // the row shows "shown: false".
    expect(byKey["ui.disable_automated_welcome_screen"]).toBe(false)
    expect(byKey["ui.disable_release_notes"]).toBe(false)
  })

  // The first-load rows are the only inverted ones. The row is phrased
  // positively ("Show the welcome screen") while the config field is a negative
  // (`disable_automated_welcome_screen`), so `read` must return the SHOWN value
  // and `buildWrites` must flip it back exactly once. A missing `inverted` flag
  // here would silently save the opposite of what the switch shows.
  it("marks the two first-load rows as inverted and shows them positively", () => {
    const byKey = Object.fromEntries(
      allSettingDescriptors().map((d) => [d.key, d]),
    )
    for (const key of [
      "ui.disable_automated_welcome_screen",
      "ui.disable_release_notes",
    ]) {
      const d = byKey[key]
      expect(d.inverted, `${key} must be inverted`).toBe(true)
      expect(d.control.kind, key).toBe("bool")
      // Shown positively: the label asks to SHOW something, and the shown
      // default is true (both config flags default to false = not disabled).
      expect(d.label.toLowerCase(), key).toContain("show")
      expect(d.default, key).toBe(true)
      // The description must say the menu entry still works, because that is the
      // whole distinction between "disable the automatic screen" and "remove the
      // feature".
      expect(d.description.toLowerCase(), key).toContain("still opens it")
    }
    // Nothing else is inverted; inversion is a deliberate exception, not a habit.
    const inverted = allSettingDescriptors()
      .filter((d) => d.inverted)
      .map((d) => d.key)
      .sort()
    expect(inverted).toEqual([
      "ui.disable_automated_welcome_screen",
      "ui.disable_release_notes",
    ])
  })

  it("read() falls back to the terminal font defaults on an older bootstrap", () => {
    const bare = { ...sampleBootstrap } as Partial<Bootstrap>
    delete bare.terminal_font_family
    delete bare.terminal_font_size
    const byKey = Object.fromEntries(
      allSettingDescriptors().map((d) => [d.key, d.read(bare as Bootstrap)]),
    )
    expect(byKey["ui.terminal_font_family"]).toBe("")
    expect(byKey["ui.terminal_font_size"]).toBe(14)
  })

  it("read() shows the long-paste threshold as OFF on a server that never published it", () => {
    // The one number in this file whose absent value is NOT its default. An
    // older server publishes nothing, `TerminalPane` reads that as `0` and
    // files nothing away, and `bootstrapApi.ts` documents absent-means-off. A
    // dialog reading the same absence as 1000 would show a threshold that is
    // not in force, and a user "confirming" it would silently switch the
    // feature ON.
    const bare = { ...sampleBootstrap } as Partial<Bootstrap>
    delete bare.upload_pasted_text_chars
    const byKey = Object.fromEntries(
      allSettingDescriptors().map((d) => [d.key, d.read(bare as Bootstrap)]),
    )
    expect(byKey["ui.upload_pasted_text_chars"]).toBe(0)
  })

  it("read() reports the long-paste threshold the server actually published", () => {
    expect(
      allSettingDescriptors()
        .find((d) => d.key === "ui.upload_pasted_text_chars")
        ?.read({ ...sampleBootstrap, upload_pasted_text_chars: 2500 }),
    ).toBe(2500)
  })

  it("names the server's floor on the long-paste row, since the control cannot enforce it", () => {
    // The control's `min` is 0, because 0 is the off switch and bounding the
    // input at the floor would make it unreachable. That leaves a real gap: a
    // user typing 50 is clamped up to 200 by the server with a warning that
    // only lands in `dux.log`, and the browser would otherwise say nothing at
    // all about it.
    const d = allSettingDescriptors().find(
      (d) => d.key === "ui.upload_pasted_text_chars",
    )
    expect(d?.description).toContain("200")
  })

  it("read() falls back to showing both first-load screens on an older bootstrap", () => {
    const bare = { ...sampleBootstrap } as Partial<Bootstrap>
    delete bare.disable_automated_welcome_screen
    delete bare.disable_release_notes
    const byKey = Object.fromEntries(
      allSettingDescriptors().map((d) => [d.key, d.read(bare as Bootstrap)]),
    )
    expect(byKey["ui.disable_automated_welcome_screen"]).toBe(true)
    expect(byKey["ui.disable_release_notes"]).toBe(true)
  })

  // CROSS-LANGUAGE PIN. The keys this modal can PATCH live twice: here, and in
  // `SettingsBody` in `crates/dux-web/src/config_routes.rs`. There is no codegen
  // path between them, so both halves are pinned by a loud test instead: this
  // one, and `set_settings_accepts_every_key_the_modal_can_send` on the server,
  // which PATCHes exactly this key set and asserts every value lands.
  //
  // This asserts SET EQUALITY, not a spot check, because the `patchSettings`
  // body type cannot catch a drift (see the comment on it in `configApi.ts`).
  // Adding a `writeTarget: "settings"` descriptor fails here until the server's
  // `SettingsBody` grows the same key, and `deny_unknown_fields` means a key the
  // server lacks is a 400 at runtime, not a silent no-op.
  it("the settings-PATCH key set matches the server's accepted fields", () => {
    const sent = allSettingDescriptors()
      .filter((d) => d.writeTarget === "settings")
      .map((d) => d.key)
      .sort()
    expect(sent).toEqual([
      "capabilities.hyperlinks",
      "capabilities.web_notifications",
      "defaults.enable_randomized_pet_name_by_default",
      "defaults.provider",
      "ui.always_show_tab_strip",
      "ui.attention_grace_seconds",
      "ui.attention_indicator",
      "ui.attention_on_bell",
      "ui.auto_reopen_agents",
      "ui.compose_bar",
      "ui.copy_on_select",
      "ui.disable_automated_welcome_screen",
      "ui.disable_release_notes",
      "ui.mobile_accessory_bar",
      "ui.mobile_top_bar",
      "ui.pr_banner_position",
      "ui.status_clear_seconds",
      "ui.terminal_font_family",
      "ui.terminal_font_size",
      "ui.upload_pasted_text_chars",
      "ui.upload_write_gitignore",
    ])
  })

  // The server's `SettingsBody` also accepts `ui.show_changes_pane`, which this
  // modal deliberately never sends through the PATCH: its row is bespoke because
  // the store keeps an optimistic override for it and routes it to the dedicated
  // Changes-pane endpoint. The asymmetry is intentional, so it is pinned rather
  // than left to look like an oversight in the set above.
  it("routes show_changes_pane to the dedicated Changes-pane endpoint", () => {
    const d = allSettingDescriptors().find((d) => d.key === "ui.show_changes_pane")
    expect(d?.writeTarget).toBe("changesPane")
  })

  // GitHub integration must NOT ride the generic settings PATCH. Flipping it
  // arms/disarms background PR syncing and clears cached statuses, and that
  // logic lives behind the dedicated endpoint; duplicating it into set_settings
  // would fork it. Precedent: `changesPane` is already a bespoke target for the
  // same reason.
  it("routes github_integration to the dedicated toggle endpoint", () => {
    const d = allSettingDescriptors().find((d) => d.key === "ui.github_integration")
    expect(d?.writeTarget).toBe("github")
  })

  // The pet-name default is a plain field write with no side effects, so it
  // rides the generic PATCH (which grew a `defaults` group for it).
  it("routes the pet-name default through the generic settings PATCH", () => {
    const d = allSettingDescriptors().find(
      (d) => d.key === "defaults.enable_randomized_pet_name_by_default",
    )
    expect(d?.writeTarget).toBe("settings")
  })

  it("read() falls back to the documented default on an older bootstrap missing the field", () => {
    const bare = { ...sampleBootstrap } as Partial<Bootstrap>
    delete bare.attention_indicator
    delete bare.attention_on_bell
    delete bare.global_default_provider
    delete bare.compose_bar
    delete bare.mobile_top_bar
    delete bare.mobile_accessory_bar
    delete bare.auto_reopen_agents
    const byKey = Object.fromEntries(
      allSettingDescriptors().map((d) => [d.key, d.read(bare as Bootstrap)]),
    )
    expect(byKey["ui.attention_indicator"]).toBe(true)
    expect(byKey["ui.attention_on_bell"]).toBe(true)
    expect(byKey["defaults.provider"]).toBe("claude")
    // The three-way mode's documented default, not a boolean.
    expect(byKey["ui.compose_bar"]).toBe("auto")
    expect(byKey["ui.mobile_top_bar"]).toBe(true)
    expect(byKey["ui.mobile_accessory_bar"]).toBe(true)
    // Unlike the mobile-bar preferences, the auto-reopen fallback is FALSE
    // (the config default).
    expect(byKey["ui.auto_reopen_agents"]).toBe(false)
  })

  // Escape-hatch truth: each mobile-bar description must name BOTH restore
  // routes (the show-bars button that renders below the terminal — inside the
  // compose bar when it is on, in its own minimal row when it is off — and
  // this Preferences dialog) rather than leaving the user to rediscover them.
  // Deliberately NOT "the compose bar's button": that wording was false with
  // the compose bar disabled.
  it("both mobile-bar descriptions name the restore routes", () => {
    for (const key of ["ui.mobile_top_bar", "ui.mobile_accessory_bar"]) {
      const d = allSettingDescriptors().find((x) => x.key === key)
      expect(d?.writeTarget, key).toBe("settings")
      expect(d?.default, key).toBe(true)
      expect(d?.description, key).toContain("below the terminal")
      expect(d?.description, key).toContain("Preferences")
    }
  })

  // The global default-provider row's options aren't known statically: they
  // come from the live bootstrap's `available_providers`, so the client can
  // never offer a provider name the server doesn't have configured. It rides
  // the generic settings PATCH (a plain field write, no side effects).
  it("sources the default-provider row's options from available_providers, not a static list", () => {
    const d = allSettingDescriptors().find((d) => d.key === "defaults.provider")
    expect(d?.control).toEqual({ kind: "enum-dynamic", source: "available_providers" })
    expect(d?.writeTarget).toBe("settings")
  })

  it("number controls declare a zeroMeaning where the config documents one", () => {
    const byKey = Object.fromEntries(allSettingDescriptors().map((d) => [d.key, d]))
    for (const key of ["ui.status_clear_seconds", "ui.attention_grace_seconds"]) {
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
})
