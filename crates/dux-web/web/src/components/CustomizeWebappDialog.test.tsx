// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import type { ReactNode } from "react"

import type { Bootstrap } from "@/lib/bootstrapApi"
import type { DuxState } from "@/lib/store"

// Override `useDux` so the dialog reads our seeded bootstrap, and replace the
// store actions the dialog dispatches with spies so we can assert the exact
// body each posts. The rest of the real store exports stay intact.
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    setInstanceIdentity: vi.fn(),
    closeCustomizeWebapp: vi.fn(),
    saveSettings: vi.fn(),
    setChangesPaneVisibility: vi.fn(),
  }
})

// The GitHub row does NOT ride the generic settings PATCH: it routes to the
// dedicated endpoint that also drives the engine's PR-sync side effects.
vi.mock("@/lib/configApi", () => ({
  configApi: { toggleGithubIntegration: vi.fn() },
}))

// The real tooltip only mounts its popup on hover and needs a ResizeObserver
// that jsdom lacks; render its trigger children directly so the swatch
// buttons exist.
vi.mock("@/components/SimpleTooltip", () => ({
  SimpleTooltip: ({ children }: { children: ReactNode }) => <>{children}</>,
}))

// The real store boots on import (localStorage + bootstrap fetch). jsdom
// doesn't provide those as bare globals, so stub them before the component
// loads.
// base-ui's Select (and other Radix-style popups) probe pointer-capture and
// layout APIs jsdom doesn't implement; without these stubs the trigger's
// pointerdown handler throws internally and the popup never opens.
function installPointerCaptureStubs() {
  const proto = Element.prototype as unknown as {
    hasPointerCapture?: () => boolean
    setPointerCapture?: () => void
    releasePointerCapture?: () => void
    scrollIntoView?: () => void
  }
  proto.hasPointerCapture ??= () => false
  proto.setPointerCapture ??= () => {}
  proto.releasePointerCapture ??= () => {}
  proto.scrollIntoView ??= () => {}
}
installPointerCaptureStubs()

function installBootStubs() {
  const mem = new Map<string, string>()
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => mem.get(k) ?? null,
    setItem: (k: string, v: string) => void mem.set(k, String(v)),
    removeItem: (k: string) => void mem.delete(k),
    clear: () => mem.clear(),
  })
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.reject(new Error("offline test"))),
  )
}
installBootStubs()
const { CustomizeWebappDialog } = await import("./CustomizeWebappDialog")
const store = await import("@/lib/store")
const setInstanceIdentity = vi.mocked(store.setInstanceIdentity)
const closeCustomizeWebapp = vi.mocked(store.closeCustomizeWebapp)
const saveSettings = vi.mocked(store.saveSettings)
const setChangesPaneVisibility = vi.mocked(store.setChangesPaneVisibility)
const { configApi } = await import("@/lib/configApi")
const toggleGithubIntegration = vi.mocked(configApi.toggleGithubIntegration)

const fullBootstrap: Bootstrap = {
  available_providers: [],
  macros: [],
  welcome_tips: [],
  dux_version: "v0.0.0",
  randomize_agent_names_by_default: false,
  gh_available: false,
  github_integration: false,
  copy_on_select: true,
  attention_grace_seconds: 3,
  web_notifications: true,
  hyperlinks: true,
  clipboard_passthrough: "focused",
  pr_banner_position: "bottom",
  agent_scrollback_lines: 10000,
  show_changes_pane: true,
  global_env: {},
  status_clear_seconds: 6,
  title: "old instance",
  favicon: "",
  agent_tabs_max: 20,
  always_show_tab_strip: false,
  attention_indicator: true,
  attention_on_bell: true,
}

function seed(overrides: Partial<Bootstrap> = {}) {
  mockState = {
    customizeWebappOpen: true,
    changesPaneOverride: null,
    bootstrap: { ...fullBootstrap, ...overrides },
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
  setInstanceIdentity.mockClear().mockResolvedValue(true)
  closeCustomizeWebapp.mockClear()
  saveSettings.mockClear().mockResolvedValue(true)
  setChangesPaneVisibility.mockClear().mockResolvedValue(true)
  toggleGithubIntegration.mockClear().mockResolvedValue(undefined as never)
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("CustomizeWebappDialog", () => {
  it("surface groups render the cross-surface caption", () => {
    seed()
    render(<CustomizeWebappDialog />)

    expect(screen.getByText(/This browser \(Web\)/)).toBeTruthy()
    expect(screen.getByText(/Both surfaces/)).toBeTruthy()
  })

  it("renders a Switch for each bool setting", async () => {
    seed()
    render(<CustomizeWebappDialog />)

    // Derived from the descriptors rather than hardcoded, so adding a bool row
    // doesn't fail this test for the wrong reason.
    const { allSettingDescriptors } = await import("@/lib/settingsDescriptors")
    const bools = allSettingDescriptors().filter((d) => d.control.kind === "bool")
    expect(bools.length).toBeGreaterThan(0)
    expect(screen.getAllByRole("switch").length).toBe(bools.length)
  })

  it("renders a number input for u64 settings and shows the zero-value hint", () => {
    seed()
    render(<CustomizeWebappDialog />)

    const input = screen.getByLabelText("Status message auto-clear") as HTMLInputElement
    expect(input.type).toBe("number")
    expect(input.value).toBe("6")
    expect(screen.getByText(/never auto-clear/i)).toBeTruthy()
  })

  it("renders a select for enum settings", () => {
    seed()
    render(<CustomizeWebappDialog />)

    // One enum row: pr_banner_position.
    expect(screen.getAllByRole("combobox").length).toBe(1)
  })

  it("shows the documented default for each row", () => {
    seed()
    render(<CustomizeWebappDialog />)

    expect(screen.getByText(/Default: dux\b/)).toBeTruthy()
    expect(screen.getByText(/Default: 6 seconds/)).toBeTruthy()
    expect(screen.getAllByText(/Default: On/).length).toBeGreaterThan(0)
  })

  it("keeps title + favicon on the instance-identity endpoint", async () => {
    seed()
    render(<CustomizeWebappDialog />)

    const input = screen.getByLabelText("Instance name") as HTMLInputElement
    fireEvent.change(input, { target: { value: "prod dux" } })
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    expect(setInstanceIdentity).toHaveBeenCalledWith({ title: "prod dux" })
    expect(saveSettings).not.toHaveBeenCalled()
  })

  it("calls saveSettings with only the changed keys on Save", async () => {
    seed()
    render(<CustomizeWebappDialog />)

    // "Copy on select", NOT "Show the Changes pane": that one is bespoke and
    // is covered by its own dedicated test below.
    fireEvent.click(screen.getByLabelText("Copy on select"))
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    expect(saveSettings).toHaveBeenCalledTimes(1)
    expect(setInstanceIdentity).not.toHaveBeenCalled()
    expect(setChangesPaneVisibility).not.toHaveBeenCalled()
    const [patch] = saveSettings.mock.calls[0]
    const uiKeys = Object.keys(patch.ui ?? {})
    const capKeys = Object.keys(patch.capabilities ?? {})
    expect(uiKeys.length + capKeys.length).toBe(1)
  })

  it("saving the Changes-pane toggle calls setChangesPaneVisibility, not saveSettings, and the row reflects an active override", async () => {
    seed({ show_changes_pane: true })
    mockState.changesPaneOverride = false // a concurrent client already flipped it.
    render(<CustomizeWebappDialog />)

    // The row reflects the store's override-aware effective value (false),
    // not the raw (stale) bootstrap value (true).
    const paneSwitch = screen.getByLabelText("Show the Changes pane")
    expect(paneSwitch.getAttribute("aria-checked")).toBe("false")

    fireEvent.click(paneSwitch)
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    expect(setChangesPaneVisibility).toHaveBeenCalledWith(true)
    expect(saveSettings).not.toHaveBeenCalled()
  })

  it("does not persist anything when Save is clicked without touching a field", async () => {
    seed()
    render(<CustomizeWebappDialog />)

    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    expect(setInstanceIdentity).not.toHaveBeenCalled()
    expect(saveSettings).not.toHaveBeenCalled()
  })

  it("disables the form while a save is in flight", async () => {
    seed()
    let resolveWrite!: (v: boolean) => void
    setInstanceIdentity.mockReturnValue(
      new Promise<boolean>((r) => {
        resolveWrite = r
      }),
    )
    render(<CustomizeWebappDialog />)

    const input = screen.getByLabelText("Instance name") as HTMLInputElement
    fireEvent.change(input, { target: { value: "renamed" } })
    const saveButton = screen.getByRole("button", { name: "Save" }) as HTMLButtonElement
    fireEvent.click(saveButton)

    expect(saveButton.disabled).toBe(true)
    resolveWrite(true)
    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
  })

  it("a double-click on Save fires the persists once", async () => {
    seed()
    render(<CustomizeWebappDialog />)

    const input = screen.getByLabelText("Instance name") as HTMLInputElement
    fireEvent.change(input, { target: { value: "renamed" } })
    const saveButton = screen.getByRole("button", { name: "Save" })
    fireEvent.click(saveButton)
    fireEvent.click(saveButton)

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    expect(setInstanceIdentity).toHaveBeenCalledTimes(1)
  })

  it("stays open when a persist fails so the user can retry", async () => {
    seed()
    setInstanceIdentity.mockResolvedValue(false)
    render(<CustomizeWebappDialog />)

    const input = screen.getByLabelText("Instance name") as HTMLInputElement
    fireEvent.change(input, { target: { value: "renamed" } })
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(setInstanceIdentity).toHaveBeenCalled())
    await waitFor(() =>
      expect(
        (screen.getByRole("button", { name: "Save" }) as HTMLButtonElement)
          .disabled,
      ).toBe(false),
    )
    expect(closeCustomizeWebapp).not.toHaveBeenCalled()
  })

  it("Escape closes the dialog when no persist is in flight", () => {
    seed()
    render(<CustomizeWebappDialog />)

    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" })
    expect(closeCustomizeWebapp).toHaveBeenCalled()
  })

  it("ignores Escape while a persist is in flight, then closes when it settles", async () => {
    seed()
    let resolveWrite!: (v: boolean) => void
    setInstanceIdentity.mockReturnValue(
      new Promise<boolean>((r) => {
        resolveWrite = r
      }),
    )
    render(<CustomizeWebappDialog />)

    const input = screen.getByLabelText("Instance name") as HTMLInputElement
    fireEvent.change(input, { target: { value: "renamed" } })
    fireEvent.click(screen.getByRole("button", { name: "Save" }))
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" })
    expect(closeCustomizeWebapp).not.toHaveBeenCalled()

    resolveWrite(true)
    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
  })

  it("an untouched switch tracks a concurrent client's toggle and never writes it back", async () => {
    seed({ show_changes_pane: true })
    const { rerender } = render(<CustomizeWebappDialog />)
    const paneSwitch = screen.getByLabelText("Show the Changes pane")
    expect(paneSwitch.getAttribute("aria-checked")).toBe("true")

    seed({ show_changes_pane: false })
    rerender(<CustomizeWebappDialog />)
    expect(
      screen.getByLabelText("Show the Changes pane").getAttribute("aria-checked"),
    ).toBe("false")

    fireEvent.click(screen.getByRole("button", { name: "Save" }))
    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    expect(saveSettings).not.toHaveBeenCalled()
  })

  it("selects the empty favicon when the Original swatch is chosen", async () => {
    seed({ favicon: "amber" })
    render(<CustomizeWebappDialog />)

    fireEvent.click(screen.getByRole("button", { name: "Blue" }))
    fireEvent.click(screen.getByRole("button", { name: "Original" }))
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    expect(setInstanceIdentity).toHaveBeenCalledWith({ favicon: "" })
  })

  it("sizes favicon swatch buttons explicitly instead of deriving size from the grid column", () => {
    seed()
    render(<CustomizeWebappDialog />)

    // Regression guard: the swatch control's wrapper (`SettingRow`'s
    // `shrink-0` div) is an auto-width flex child, so a swatch that derives
    // its size from `aspect-square` inside a `grid-cols-6` column blows up
    // to that column's shrink-to-fit width instead of a fixed size. Every
    // swatch button must carry an explicit fixed square size (`size-10`) and
    // must NOT rely on `aspect-square` for sizing.
    const original = screen.getByRole("button", { name: "Original" })
    expect(original.className).toMatch(/\bsize-10\b/)
    expect(original.className).not.toMatch(/\baspect-square\b/)

    const blue = screen.getByRole("button", { name: "Blue" })
    expect(blue.className).toMatch(/\bsize-10\b/)
    expect(blue.className).not.toMatch(/\baspect-square\b/)
  })

  it("resets the Web section to defaults and persists exactly the changed keys", async () => {
    seed({ title: "prod dux", favicon: "amber", copy_on_select: false })
    render(<CustomizeWebappDialog />)

    fireEvent.click(screen.getAllByRole("button", { name: /Reset section to defaults/i })[0])

    await waitFor(() => expect(setInstanceIdentity).toHaveBeenCalled())
    expect(setInstanceIdentity).toHaveBeenCalledWith({ title: "", favicon: "" })
    expect(saveSettings).toHaveBeenCalledTimes(1)
    const [patch] = saveSettings.mock.calls[0]
    // Only `copy_on_select` differed from its default in this seed. Every
    // other Web-section key (show_changes_pane is bespoke and already at its
    // default, web_notifications is already at its default) must be absent,
    // and no `both`-group key should leak in from a full-descriptor
    // reset.
    expect(patch.ui).toEqual({ copy_on_select: true })
    expect(patch.capabilities ?? {}).toEqual({})
    expect(setChangesPaneVisibility).not.toHaveBeenCalled()
  })

  it("resets the Web section's Changes-pane row through setChangesPaneVisibility when it differs from default", async () => {
    seed({ show_changes_pane: false })
    render(<CustomizeWebappDialog />)

    fireEvent.click(screen.getAllByRole("button", { name: /Reset section to defaults/i })[0])

    await waitFor(() => expect(setChangesPaneVisibility).toHaveBeenCalledWith(true))
    // show_changes_pane must never ride the generic settings PATCH.
    for (const call of saveSettings.mock.calls) {
      const [patch] = call
      expect(patch.ui ?? {}).not.toHaveProperty("show_changes_pane")
    }
  })

  it("a failed reset does not leave the controls showing defaults", async () => {
    seed({ copy_on_select: false })
    saveSettings.mockResolvedValue(false)
    render(<CustomizeWebappDialog />)

    fireEvent.click(screen.getAllByRole("button", { name: /Reset section to defaults/i })[0])

    await waitFor(() => expect(saveSettings).toHaveBeenCalled())
    await waitFor(() =>
      expect(
        (screen.getAllByRole("button", { name: /Reset section to defaults/i })[0] as HTMLButtonElement)
          .disabled,
      ).toBe(false),
    )
    // The failed write must not have applied the optimistic reset override:
    // the switch still reflects the pre-reset (non-default) value.
    expect(
      screen.getByLabelText("Copy on select").getAttribute("aria-checked"),
    ).toBe("false")
  })

  it("fires a real number-control change and sends the exact key/value, including the empty-string edge", async () => {
    seed()
    render(<CustomizeWebappDialog />)

    const input = screen.getByLabelText("Status message auto-clear") as HTMLInputElement
    fireEvent.change(input, { target: { value: "42" } })
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    const [patch] = saveSettings.mock.calls[0]
    expect(patch.ui).toEqual({ status_clear_seconds: 42 })
  })

  it("does not send 0 when a number field is emptied and saved", async () => {
    seed()
    render(<CustomizeWebappDialog />)

    const input = screen.getByLabelText("Status message auto-clear") as HTMLInputElement
    fireEvent.change(input, { target: { value: "42" } })
    fireEvent.change(input, { target: { value: "" } })
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    // The last committed value (42) stands, the empty keystroke never
    // recorded a 0 override.
    const [patch] = saveSettings.mock.calls[0]
    expect(patch.ui).toEqual({ status_clear_seconds: 42 })
  })

  it("clamps a value above max client-side before it reaches the sent payload", async () => {
    seed()
    render(<CustomizeWebappDialog />)

    // ui.attention_grace_seconds caps at 300.
    const input = screen.getByLabelText("Attention grace") as HTMLInputElement
    fireEvent.change(input, { target: { value: "99999" } })
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    const [patch] = saveSettings.mock.calls[0]
    expect(patch.ui).toEqual({ attention_grace_seconds: 300 })
  })

  it("fires a real select change and sends the exact key/value", async () => {
    seed()
    render(<CustomizeWebappDialog />)

    // The "both" group's enum row: PR banner position (top/bottom). base-ui's
    // SelectItem only commits a plain `click` as a real (non-virtual) mouse
    // selection when it was preceded by a `pointerdown` on the same item
    // (see `SelectItem.js`'s `allowMouseSelectionRef`), so a bare
    // `fireEvent.click` alone is silently ignored as an untrusted click.
    const trigger = screen.getByLabelText("PR banner position")
    fireEvent.click(trigger)
    await waitFor(() => expect(trigger.getAttribute("aria-expanded")).toBe("true"))
    const option = await screen.findByRole("option", { name: "Top" })
    fireEvent.pointerDown(option, { pointerType: "mouse" })
    fireEvent.click(option)
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    const [patch] = saveSettings.mock.calls[0]
    expect(patch.ui).toEqual({ pr_banner_position: "top" })
  })

  // ── Rows rehomed from the deleted web command palette ──────────────────────

  it("renders a GitHub integration row", () => {
    seed()
    render(<CustomizeWebappDialog />)
    expect(screen.getByLabelText("GitHub integration")).toBeTruthy()
  })

  it("saves GitHub integration through the dedicated endpoint, not the settings PATCH", async () => {
    seed({ github_integration: false })
    render(<CustomizeWebappDialog />)

    fireEvent.click(screen.getByLabelText("GitHub integration"))
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    expect(toggleGithubIntegration).toHaveBeenCalledTimes(1)
    // The generic PATCH must never carry this field: the endpoint owns the
    // PR-sync side effects, and set_settings would only write the flag.
    for (const [patch] of saveSettings.mock.calls) {
      expect(patch.ui ?? {}).not.toHaveProperty("github_integration")
    }
  })

  // THE HAZARD PIN. The endpoint is a blind read-and-flip while this modal saves
  // explicit values; they only agree because an unchanged row is never sent. If
  // `persist` ever writes unconditionally, this catches it before it silently
  // inverts the user's setting.
  it("does not call the GitHub endpoint when the row is unchanged", async () => {
    seed({ github_integration: true })
    render(<CustomizeWebappDialog />)

    // Touch a DIFFERENT row, so there is a save to perform, then flip the GitHub
    // row twice so it is "touched" but lands back on its original value.
    fireEvent.click(screen.getByLabelText("Copy on select"))
    fireEvent.click(screen.getByLabelText("GitHub integration"))
    fireEvent.click(screen.getByLabelText("GitHub integration"))
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    expect(saveSettings).toHaveBeenCalledTimes(1)
    expect(toggleGithubIntegration).not.toHaveBeenCalled()
  })

  // The description text is adapted from a config.toml comment and contains
  // markdown-style backtick spans (e.g. "the `gh` CLI"). These must render as
  // actual <code> elements, not literal backtick characters.
  it("renders the GitHub integration row's backtick spans as code elements", () => {
    seed()
    render(<CustomizeWebappDialog />)

    const row = screen.getByLabelText("GitHub integration").closest(".flex-col.gap-2")
    expect(row).not.toBeNull()
    const codeEls = row!.querySelectorAll("code")
    expect(codeEls.length).toBeGreaterThan(0)
    expect(Array.from(codeEls).some((el) => el.textContent === "gh")).toBe(true)
    expect(row!.textContent).not.toContain("`")
  })

  // ── Browser-notification permission (rehomed from the palette) ────────────
  //
  // This was the palette's only client-side item and the ONLY way to grant
  // notification permission (dux never auto-prompts). It is not a config value,
  // so it is not a SettingDescriptor; it lives next to the "Desktop
  // notifications" row it depends on, and appears only while permission can
  // still be asked for.

  function stubNotifications(permission: NotificationPermission | null) {
    if (permission === null) {
      vi.stubGlobal("Notification", undefined)
      return vi.fn()
    }
    const requestPermission = vi.fn(() => Promise.resolve("granted" as const))
    vi.stubGlobal("Notification", { permission, requestPermission })
    return requestPermission
  }

  it("offers the browser-notification permission row only when permission is default", () => {
    stubNotifications("default")
    seed()
    render(<CustomizeWebappDialog />)
    expect(
      screen.getByRole("button", { name: /enable browser notifications/i }),
    ).toBeTruthy()
  })

  it("hides the permission row once permission is granted", () => {
    stubNotifications("granted")
    seed()
    render(<CustomizeWebappDialog />)
    expect(
      screen.queryByRole("button", { name: /enable browser notifications/i }),
    ).toBeNull()
  })

  it("hides the permission row when permission was denied", () => {
    stubNotifications("denied")
    seed()
    render(<CustomizeWebappDialog />)
    expect(
      screen.queryByRole("button", { name: /enable browser notifications/i }),
    ).toBeNull()
  })

  it("hides the permission row when the browser has no Notification API", () => {
    stubNotifications(null)
    seed()
    render(<CustomizeWebappDialog />)
    expect(
      screen.queryByRole("button", { name: /enable browser notifications/i }),
    ).toBeNull()
  })

  it("hides the permission row when desktop notifications are disabled in config", () => {
    stubNotifications("default")
    seed({ web_notifications: false })
    render(<CustomizeWebappDialog />)
    expect(
      screen.queryByRole("button", { name: /enable browser notifications/i }),
    ).toBeNull()
  })

  it("requests permission when the row is used, and never before", async () => {
    const requestPermission = stubNotifications("default")
    seed()
    render(<CustomizeWebappDialog />)
    // dux never auto-prompts: merely opening the dialog must ask for nothing.
    expect(requestPermission).not.toHaveBeenCalled()

    fireEvent.click(
      screen.getByRole("button", { name: /enable browser notifications/i }),
    )
    await waitFor(() => expect(requestPermission).toHaveBeenCalledTimes(1))
  })

  it("renders a randomized pet-name default row", () => {
    seed()
    render(<CustomizeWebappDialog />)
    expect(
      screen.getByLabelText("Random pet-name default for new agents"),
    ).toBeTruthy()
  })

  it("saves the pet-name default through the settings PATCH", async () => {
    seed({ randomize_agent_names_by_default: false })
    render(<CustomizeWebappDialog />)

    fireEvent.click(
      screen.getByLabelText("Random pet-name default for new agents"),
    )
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    expect(saveSettings).toHaveBeenCalledTimes(1)
    expect(toggleGithubIntegration).not.toHaveBeenCalled()
    const [patch] = saveSettings.mock.calls[0]
    expect(patch.defaults).toEqual({
      enable_randomized_pet_name_by_default: true,
    })
  })
})
