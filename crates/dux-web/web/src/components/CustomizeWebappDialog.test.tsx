// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
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
    setAsidePreferencesDraft: vi.fn(),
    openConfigEditor: vi.fn(),
  }
})

let phone = false
vi.mock("@/hooks/use-mobile", () => ({
  MOBILE_BREAKPOINT: 768,
  isMobileViewport: () => phone,
  useIsMobile: () => phone,
}))

// The GitHub row does NOT ride the generic settings PATCH: it routes to the
// dedicated endpoint that also drives the engine's PR-sync side effects.
vi.mock("@/lib/configApi", () => ({
  configApi: { toggleGithubIntegration: vi.fn(), setTailscaleMode: vi.fn() },
}))

// The Tailscale row's reply is a sentence, and its tone decides which raiser
// it goes to. `lib/notify` is the one place a toast is raised, so it is the one
// place to spy on.
vi.mock("@/lib/notify", () => ({
  notifyError: vi.fn(),
  notifyInfo: vi.fn(),
  notifySuccess: vi.fn(),
  notifyWarning: vi.fn(),
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
  vi.stubGlobal(
    "matchMedia",
    vi.fn((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    })),
  )
}
installBootStubs()
const { CustomizeWebappDialog } = await import("./CustomizeWebappDialog")
const store = await import("@/lib/store")
const setInstanceIdentity = vi.mocked(store.setInstanceIdentity)
const closeCustomizeWebapp = vi.mocked(store.closeCustomizeWebapp)
const saveSettings = vi.mocked(store.saveSettings)
const setChangesPaneVisibility = vi.mocked(store.setChangesPaneVisibility)
const setAsidePreferencesDraft = vi.mocked(store.setAsidePreferencesDraft)
const openConfigEditor = vi.mocked(store.openConfigEditor)
const { openModals } = await import("@/lib/modalHistory")
const { SETTING_TOPICS, allSettingDescriptors } = await import("@/lib/settingsDescriptors")
const { configApi } = await import("@/lib/configApi")
const toggleGithubIntegration = vi.mocked(configApi.toggleGithubIntegration)
const setTailscaleMode = vi.mocked(configApi.setTailscaleMode)
const notify = await import("@/lib/notify")
const notifyInfo = vi.mocked(notify.notifyInfo)
const notifyWarning = vi.mocked(notify.notifyWarning)

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
  // Stated rather than left absent: absence is an older server that never
  // published it, which a test of its own covers.
  upload_pasted_text_chars: 4000,
}

function seed(overrides: Partial<Bootstrap> = {}) {
  mockState = {
    customizeWebappOpen: true,
    changesPaneOverride: null,
    bootstrap: { ...fullBootstrap, ...overrides },
  } as unknown as DuxState
}

// Shows the topic a setting is filed under, then returns its control.
function field(label: string): HTMLElement {
  const d = allSettingDescriptors().find((x) => x.label === label)
  if (!d) throw new Error(`no setting ${label}`)
  topic(SETTING_TOPICS.find((t) => t.id === d.topic)!.label)
  return screen.getByLabelText(label)
}

// Presses a topic, or "Changed from default", in the side list.
function topic(name: string) {
  fireEvent.click(screen.getByRole("button", { name: new RegExp(`^${name}`) }))
}

// Lets the dialog's own opening focus land, which base-ui places a moment
// after the dialog mounts.
async function opened() {
  await act(() => new Promise((resolve) => setTimeout(resolve, 50)))
}

// The browser's Back: the store closes the newest modal holding an entry.
async function back() {
  await act(async () => {
    openModals().at(-1)!.close()
  })
  await act(() => new Promise((resolve) => setTimeout(resolve, 20)))
}

beforeEach(() => {
  installBootStubs()
  phone = false
  setAsidePreferencesDraft.mockClear()
  openConfigEditor.mockClear()
  setInstanceIdentity.mockClear().mockResolvedValue(true)
  closeCustomizeWebapp.mockClear()
  saveSettings.mockClear().mockResolvedValue(true)
  setChangesPaneVisibility.mockClear().mockResolvedValue(true)
  toggleGithubIntegration.mockClear().mockResolvedValue(undefined as never)
  setTailscaleMode.mockClear().mockResolvedValue({
    mode: "no",
    warning: false,
    message: "saved",
  })
  notifyInfo.mockClear()
  notifyWarning.mockClear()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("CustomizeWebappDialog", () => {
  it("is titled as its menu entry is", () => {
    seed()
    render(<CustomizeWebappDialog />)
    expect(screen.getByRole("dialog", { name: "Preferences" })).toBeTruthy()
  })

  it("lists the topics with their counts, and says on each row where it applies", () => {
    seed()
    render(<CustomizeWebappDialog />)

    const nav = screen.getByRole("navigation", { name: "Preferences topics" })
    expect(within(nav).getAllByRole("button").map((b) => b.textContent)).toEqual([
      "Changed from default2, 2 settings",
      "Appearance, some changed from default7, 7 settings",
      "Typing and touch6, 6 settings",
      "Attention and messages5, 5 settings",
      "New agents and startup, some changed from default6, 6 settings",
      "Sign-in and network2, 2 settings",
      "Terminal app3, 3 settings",
    ])
    // Appearance is on screen first; a row in it applies to this browser.
    const row = screen.getByLabelText("Instance name").closest("[data-setting-row]")!
    expect(row.textContent).toContain("This browser")
    expect(field("Attention grace").closest("[data-setting-row]")!.textContent).toContain(
      "Web and terminal app",
    )
  })

  it("renders a Switch for each bool setting, in its topic", () => {
    seed()
    render(<CustomizeWebappDialog />)

    // Derived from the descriptors rather than hardcoded, so adding a bool row
    // doesn't fail this test for the wrong reason.
    let switches = 0
    for (const t of SETTING_TOPICS) {
      topic(t.label)
      switches += screen.queryAllByRole("switch").length
    }
    const bools = allSettingDescriptors().filter((d) => d.control.kind === "bool")
    expect(bools.length).toBeGreaterThan(0)
    expect(switches).toBe(bools.length)
  })

  it("renders a number input for u64 settings and shows the zero-value hint", () => {
    seed()
    render(<CustomizeWebappDialog />)

    const input = field("Status message auto-clear") as HTMLInputElement
    expect(input.type).toBe("number")
    expect(input.value).toBe("6")
    expect(screen.getByText(/never auto-clear/i)).toBeTruthy()
  })

  it("renders a select for enum settings", () => {
    seed()
    render(<CustomizeWebappDialog />)

    // Five select rows: pr_banner_position, compose_bar, server.tailscale and
    // ui.extended_keys (static enums), and defaults.provider (enum-dynamic,
    // sourced from available_providers).
    let selects = 0
    for (const t of SETTING_TOPICS) {
      topic(t.label)
      selects += screen.queryAllByRole("combobox").length
    }
    expect(selects).toBe(5)
  })

  it("names the default on a row once the row differs from it", () => {
    seed({ title: "prod dux" })
    render(<CustomizeWebappDialog />)

    const row = (label: string) => field(label).closest("[data-setting-row]")!.textContent
    expect(row("Instance name")).toContain("This browser · default dux")
    expect(row("Status message auto-clear")).not.toContain("default")
  })

  it("keeps title + favicon on the instance-identity endpoint", async () => {
    seed()
    render(<CustomizeWebappDialog />)

    const input = field("Instance name") as HTMLInputElement
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
    fireEvent.click(field("Copy on select"))
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

  // The two first-load rows are the only INVERTED ones: the switch says "Show
  // the welcome screen" while the config field is
  // `disable_automated_welcome_screen`. If the flip in `buildWrites` were ever
  // dropped, the dialog would save the exact opposite of what the user sees:
  // silent, and invisible until someone restarted dux. Hence a test per
  // direction.
  it("saves an inverted first-load row as the negated config field", async () => {
    // Both screens currently enabled (nothing disabled), so both switches read
    // ON. Turning one OFF must send `disable_* = true`.
    seed({
      disable_automated_welcome_screen: false,
      disable_release_notes: false,
    })
    render(<CustomizeWebappDialog />)

    const welcomeSwitch = field("Show the welcome screen on a new install")
    expect(welcomeSwitch.getAttribute("aria-checked")).toBe("true")

    fireEvent.click(welcomeSwitch)
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    const [patch] = saveSettings.mock.calls[0]
    expect(patch.ui?.disable_automated_welcome_screen).toBe(true)
    // Only the touched row is sent.
    expect(patch.ui?.disable_release_notes).toBeUndefined()
  })

  it("shows an inverted first-load row as OFF when the config disables it, and saves re-enabling it", async () => {
    seed({ disable_release_notes: true })
    render(<CustomizeWebappDialog />)

    const notesSwitch = field("Show what's new after an update")
    expect(notesSwitch.getAttribute("aria-checked")).toBe("false")

    fireEvent.click(notesSwitch)
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    const [patch] = saveSettings.mock.calls[0]
    expect(patch.ui?.disable_release_notes).toBe(false)
  })

  it("saving the Changes-pane toggle calls setChangesPaneVisibility, not saveSettings, and the row reflects an active override", async () => {
    seed({ show_changes_pane: true })
    mockState.changesPaneOverride = false // a concurrent client already flipped it.
    render(<CustomizeWebappDialog />)

    // The row reflects the store's override-aware effective value (false),
    // not the raw (stale) bootstrap value (true).
    const paneSwitch = field("Show the Changes pane")
    expect(paneSwitch.getAttribute("aria-checked")).toBe("false")

    fireEvent.click(paneSwitch)
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    expect(setChangesPaneVisibility).toHaveBeenCalledWith(true)
    expect(saveSettings).not.toHaveBeenCalled()
  })

  // The terminal-keys row rides the GENERIC settings PATCH but still carries a
  // store-side optimistic override (the keys toggle in the top menu and the phone's action cluster), so like
  // the Changes-pane row its baseline must be the override-aware selector.
  // Read from the raw bootstrap, the switch would show a stale value AND
  // `buildWrites` would treat a toggle back to that stale value as a no-op and
  // silently save nothing.
  it("the touch terminal-keys row reflects an active override, and toggling back to the stale bootstrap value still writes", async () => {
    seed({ mobile_accessory_bar: true } as Partial<Bootstrap>)
    ;(
      mockState as unknown as { mobileAccessoryBarOverride: boolean | null }
    ).mobileAccessoryBarOverride = false
    render(<CustomizeWebappDialog />)

    const sw = field("Touch terminal keys")
    expect(sw.getAttribute("aria-checked")).toBe("false")

    fireEvent.click(sw)
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    expect(saveSettings).toHaveBeenCalledTimes(1)
    expect(saveSettings.mock.calls[0][0].ui).toEqual({
      mobile_accessory_bar: true,
    })
  })

  it("does not persist anything when Save is clicked without touching a field", async () => {
    seed()
    render(<CustomizeWebappDialog />)

    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    expect(setInstanceIdentity).not.toHaveBeenCalled()
    expect(saveSettings).not.toHaveBeenCalled()
    // Nothing was sent, so nothing on the wire will answer: the dialog says so.
    expect(notifyInfo).toHaveBeenCalledWith("Nothing to save: no preference was changed.")
  })

  it("leaves a real save's confirmation to the server's status", async () => {
    seed()
    render(<CustomizeWebappDialog />)

    fireEvent.click(field("Copy on select"))
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    expect(saveSettings).toHaveBeenCalledTimes(1)
    expect(notifyInfo).not.toHaveBeenCalled()
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

    const input = field("Instance name") as HTMLInputElement
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

    const input = field("Instance name") as HTMLInputElement
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

    const input = field("Instance name") as HTMLInputElement
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

    const input = field("Instance name") as HTMLInputElement
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
    const paneSwitch = field("Show the Changes pane")
    expect(paneSwitch.getAttribute("aria-checked")).toBe("true")

    seed({ show_changes_pane: false })
    rerender(<CustomizeWebappDialog />)
    expect(
      field("Show the Changes pane").getAttribute("aria-checked"),
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

    // The swatch control's wrapper (`SettingRow`'s `shrink-0` div) is an
    // auto-width flex child, so a swatch that derives
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

  it("fires a real number-control change and sends the exact key/value, including the empty-string edge", async () => {
    seed()
    render(<CustomizeWebappDialog />)

    const input = field("Status message auto-clear") as HTMLInputElement
    fireEvent.change(input, { target: { value: "42" } })
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    const [patch] = saveSettings.mock.calls[0]
    expect(patch.ui).toEqual({ status_clear_seconds: 42 })
  })

  it("does not send 0 when a number field is emptied and saved", async () => {
    seed()
    render(<CustomizeWebappDialog />)

    const input = field("Status message auto-clear") as HTMLInputElement
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
    const input = field("Attention grace") as HTMLInputElement
    fireEvent.change(input, { target: { value: "99999" } })
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    const [patch] = saveSettings.mock.calls[0]
    expect(patch.ui).toEqual({ attention_grace_seconds: 300 })
  })

  it("lets a number below the minimum stand while typing and brings it into range on leaving the field", () => {
    seed()
    render(<CustomizeWebappDialog />)

    const input = field("Terminal font size") as HTMLInputElement
    fireEvent.change(input, { target: { value: "" } })
    fireEvent.change(input, { target: { value: "1" } })
    expect(input.value).toBe("1")
    fireEvent.change(input, { target: { value: "16" } })
    expect(input.value).toBe("16")

    fireEvent.change(input, { target: { value: "2" } })
    fireEvent.blur(input)
    expect(input.value).toBe("8")
  })

  it("brings a number still out of range into range when Save is pressed without leaving the field", async () => {
    seed()
    render(<CustomizeWebappDialog />)

    const input = field("Terminal font size") as HTMLInputElement
    fireEvent.change(input, { target: { value: "3" } })
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    expect(input.value).toBe("8")
    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    const [patch] = saveSettings.mock.calls[0]
    expect(patch.ui).toEqual({ terminal_font_size: 8 })
    expect(notifyInfo).toHaveBeenCalledWith(
      "Terminal font size was saved as 8 px, not 3: it goes from 8 to 32.",
    )
  })

  it("says a number brought into range on leaving the field was saved instead of the one typed", async () => {
    seed()
    render(<CustomizeWebappDialog />)

    const input = field("Terminal font size") as HTMLInputElement
    fireEvent.change(input, { target: { value: "3" } })
    // A mouse press on Save takes focus off the field first.
    fireEvent.blur(input)
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    expect(notifyInfo).toHaveBeenCalledWith(
      "Terminal font size was saved as 8 px, not 3: it goes from 8 to 32.",
    )
  })

  it("fires a real select change and sends the exact key/value", async () => {
    seed()
    render(<CustomizeWebappDialog />)

    // The "both" group's enum row: PR banner position (top/bottom). base-ui's
    // SelectItem only commits a plain `click` as a real (non-virtual) mouse
    // selection when it was preceded by a `pointerdown` on the same item
    // (see `SelectItem.js`'s `allowMouseSelectionRef`), so a bare
    // `fireEvent.click` alone is silently ignored as an untrusted click.
    const trigger = field("PR banner position")
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

  // The global default-provider row's options come from the live bootstrap's
  // `available_providers`, never a hardcoded list, so the client can never
  // offer a provider name the server doesn't have configured.
  it("renders the default-provider row's options from available_providers and saves the chosen one under defaults", async () => {
    seed({ available_providers: ["claude", "codex"], global_default_provider: "claude" })
    render(<CustomizeWebappDialog />)

    const trigger = field("Default provider for new agents")
    fireEvent.click(trigger)
    await waitFor(() => expect(trigger.getAttribute("aria-expanded")).toBe("true"))
    expect(screen.getByRole("option", { name: "claude" })).toBeTruthy()
    const option = await screen.findByRole("option", { name: "codex" })
    fireEvent.pointerDown(option, { pointerType: "mouse" })
    fireEvent.click(option)
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    const [patch] = saveSettings.mock.calls[0]
    expect(patch.defaults).toEqual({ provider: "codex" })
  })

  // ── Rows rehomed from the deleted web command palette ──────────────────────

  it("renders a GitHub integration row", () => {
    seed()
    render(<CustomizeWebappDialog />)
    expect(field("GitHub integration")).toBeTruthy()
  })

  it("saves GitHub integration through the dedicated endpoint, not the settings PATCH", async () => {
    seed({ github_integration: false })
    render(<CustomizeWebappDialog />)

    fireEvent.click(field("GitHub integration"))
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    expect(toggleGithubIntegration).toHaveBeenCalledTimes(1)
    // The generic PATCH must never carry this field: the endpoint owns the
    // PR-sync side effects, and set_settings would only write the flag.
    for (const [patch] of saveSettings.mock.calls) {
      expect(patch.ui ?? {}).not.toHaveProperty("github_integration")
    }
  })

  // ── The Tailscale mode row ────────────────────────────────────────────────

  async function pickTailscaleMode(label: string) {
    const trigger = field("Bind your Tailscale address")
    fireEvent.click(trigger)
    await waitFor(() => expect(trigger.getAttribute("aria-expanded")).toBe("true"))
    const option = await screen.findByRole("option", { name: label })
    fireEvent.pointerDown(option, { pointerType: "mouse" })
    fireEvent.click(option)
  }

  it("saves the Tailscale mode through its own endpoint, not the settings PATCH", async () => {
    // Saving the value is only half of the row: the other half moves a live
    // listener, which the generic PATCH cannot do.
    seed({ tailscale_mode: "auto" })
    render(<CustomizeWebappDialog />)

    const trigger = field("Bind your Tailscale address")
    fireEvent.click(trigger)
    await waitFor(() => expect(trigger.getAttribute("aria-expanded")).toBe("true"))
    const option = await screen.findByRole("option", { name: "No" })
    fireEvent.pointerDown(option, { pointerType: "mouse" })
    fireEvent.click(option)
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    expect(setTailscaleMode).toHaveBeenCalledTimes(1)
    expect(setTailscaleMode).toHaveBeenCalledWith("no")
    for (const [patch] of saveSettings.mock.calls) {
      expect(patch.ui ?? {}).not.toHaveProperty("tailscale")
    }
  })

  it("leaves the Tailscale endpoint alone when the row is unchanged", async () => {
    seed({ tailscale_mode: "auto" })
    render(<CustomizeWebappDialog />)

    // Touch a different row so there is a save to perform at all.
    fireEvent.click(field("Copy on select"))
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    expect(setTailscaleMode).not.toHaveBeenCalled()
  })

  // The reply's tone is the server's, not the client's guess: "your Tailscale
  // listener would not bind" is not the same news as "saved".
  it("raises the Tailscale reply as a warning when the server grades it one", async () => {
    setTailscaleMode.mockResolvedValue({
      mode: "yes",
      warning: true,
      message: "No Tailscale address was found",
    })
    seed({ tailscale_mode: "auto" })
    render(<CustomizeWebappDialog />)

    await pickTailscaleMode("Yes")
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    expect(notifyWarning).toHaveBeenCalledWith("No Tailscale address was found")
    expect(notifyInfo).not.toHaveBeenCalled()
  })

  it("raises a plain reply as information", async () => {
    setTailscaleMode.mockResolvedValue({
      mode: "no",
      warning: false,
      message: "dux stopped serving on your Tailscale address",
    })
    seed({ tailscale_mode: "auto" })
    render(<CustomizeWebappDialog />)

    await pickTailscaleMode("No")
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    expect(notifyInfo).toHaveBeenCalledWith(
      "dux stopped serving on your Tailscale address",
    )
    expect(notifyWarning).not.toHaveBeenCalled()
  })

  const FORCED_NO_REPLY = {
    mode: "auto",
    warning: true,
    message:
      '[server] tailscale is saved as "auto". This run was started with --no-tailscale, which wins for as long as it runs, so nothing changed on the listeners; restart dux without that flag to use the saved mode.',
  }

  // ── The run-scoped lock ───────────────────────────────────────────────────

  it("keeps the Tailscale row editable on a run started with --no-tailscale and says the change waits for the next run", async () => {
    setTailscaleMode.mockResolvedValue(FORCED_NO_REPLY)
    seed({ tailscale_mode: "no", tailscale_forced_no: true })
    render(<CustomizeWebappDialog />)

    const trigger = field("Bind your Tailscale address")
    expect(trigger.hasAttribute("disabled")).toBe(false)
    const row = trigger.closest("[data-setting-row]")
    expect(row!.textContent).toContain("--no-tailscale")
    expect(row!.textContent).toContain("next time")

    await pickTailscaleMode("Auto")
    fireEvent.click(screen.getByRole("button", { name: "Save" }))
    await waitFor(() => expect(setTailscaleMode).toHaveBeenCalledWith("auto"))
    await waitFor(() => expect(notifyWarning).toHaveBeenCalledWith(FORCED_NO_REPLY.message))
  })

  it("leaves the row editable on an ordinary run", () => {
    seed({ tailscale_mode: "no", tailscale_forced_no: false })
    render(<CustomizeWebappDialog />)

    const trigger = field("Bind your Tailscale address")
    expect(trigger.hasAttribute("disabled")).toBe(false)
    expect(trigger.closest("[data-setting-row]")!.textContent).not.toContain(
      "--no-tailscale",
    )
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
    fireEvent.click(field("Copy on select"))
    fireEvent.click(field("GitHub integration"))
    fireEvent.click(field("GitHub integration"))
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

    const row = field("GitHub integration").closest("[data-setting-row]")
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
    topic("Attention and messages")
    expect(
      screen.getByRole("button", { name: /enable browser notifications/i }),
    ).toBeTruthy()
  })

  it("hides the permission row once permission is granted", () => {
    stubNotifications("granted")
    seed()
    render(<CustomizeWebappDialog />)
    topic("Attention and messages")
    expect(field("Desktop notifications")).toBeTruthy()
    expect(
      screen.queryByRole("button", { name: /enable browser notifications/i }),
    ).toBeNull()
  })

  it("hides the permission row when permission was denied", () => {
    stubNotifications("denied")
    seed()
    render(<CustomizeWebappDialog />)
    topic("Attention and messages")
    expect(field("Desktop notifications")).toBeTruthy()
    expect(
      screen.queryByRole("button", { name: /enable browser notifications/i }),
    ).toBeNull()
  })

  it("hides the permission row when the browser has no Notification API", () => {
    stubNotifications(null)
    seed()
    render(<CustomizeWebappDialog />)
    topic("Attention and messages")
    expect(field("Desktop notifications")).toBeTruthy()
    expect(
      screen.queryByRole("button", { name: /enable browser notifications/i }),
    ).toBeNull()
  })

  it("hides the permission row when desktop notifications are disabled in config", () => {
    stubNotifications("default")
    seed({ web_notifications: false })
    render(<CustomizeWebappDialog />)
    topic("Attention and messages")
    expect(field("Desktop notifications")).toBeTruthy()
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
    topic("Attention and messages")

    fireEvent.click(
      screen.getByRole("button", { name: /enable browser notifications/i }),
    )
    await waitFor(() => expect(requestPermission).toHaveBeenCalledTimes(1))
  })

  it("renders a randomized pet-name default row", () => {
    seed()
    render(<CustomizeWebappDialog />)
    expect(
      field("Random pet-name default for new agents"),
    ).toBeTruthy()
  })

  it("saves the pet-name default through the settings PATCH", async () => {
    seed({ randomize_agent_names_by_default: false })
    render(<CustomizeWebappDialog />)

    fireEvent.click(
      field("Random pet-name default for new agents"),
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

describe("CustomizeWebappDialog search", () => {
  const search = () => screen.getByLabelText("Search settings") as HTMLInputElement
  const type = (query: string) => fireEvent.change(search(), { target: { value: query } })

  it("searches every topic at once and shows each match's config key and topic", () => {
    seed()
    render(<CustomizeWebappDialog />)

    type("font")

    expect(screen.getByText("3 settings match")).toBeTruthy()
    expect(screen.getByLabelText("Terminal font")).toBeTruthy()
    expect(screen.getByLabelText("Terminal font size")).toBeTruthy()
    const nerd = screen.getByLabelText("Nerd Font icons").closest("[data-setting-row]")!
    expect(nerd.textContent).toContain("ui.nerd_font_icons")
    expect(screen.getByLabelText("Terminal font").closest("[data-setting-row]")!.textContent).toContain(
      "This browser · Appearance",
    )
    expect(screen.queryByLabelText("Instance name")).toBeNull()
  })

  it("shows only rows holding every word of the query", () => {
    seed()
    render(<CustomizeWebappDialog />)

    type("Attention BELL")

    expect(screen.getByLabelText("Attention on terminal bell")).toBeTruthy()
    expect(screen.queryByLabelText("Show attention indicator")).toBeNull()
  })

  it("finds a row by a word only its config key has, and lights it in the shown key", () => {
    seed()
    render(<CustomizeWebappDialog />)

    type("tab_reaches")

    expect(screen.getByLabelText("Send Tab to the agent")).toBeTruthy()
    expect(screen.queryByLabelText("Always show tab strip")).toBeNull()
    // The key may wrap after a dot or an underscore, so the lit run can be
    // drawn in pieces; together they are the words searched.
    const key = [...document.querySelectorAll("span.font-mono")].find(
      (el) => el.textContent === "ui.tab_reaches_agent",
    )!
    expect([...key.querySelectorAll("mark")].map((m) => m.textContent).join("")).toBe(
      "tab_reaches",
    )
  })

  it("says so when nothing matches", () => {
    seed()
    render(<CustomizeWebappDialog />)

    type("nothing-is-called-this")

    const status = screen.getByRole("status")
    expect(status.textContent).toContain("No settings match nothing-is-called-this.")
    expect(status.textContent).toContain("Edit config file…")
  })

  it("Escape clears a search first and closes the dialog only after", () => {
    seed()
    render(<CustomizeWebappDialog />)
    type("bell")

    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" })
    expect(closeCustomizeWebapp).not.toHaveBeenCalled()
    expect(search().value).toBe("")
    expect(screen.getByLabelText("Instance name")).toBeTruthy()

    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" })
    expect(closeCustomizeWebapp).toHaveBeenCalled()
  })

  it("still saves an edit made to a row the search has since hidden", async () => {
    seed()
    render(<CustomizeWebappDialog />)

    topic("Typing and touch")
    fireEvent.click(screen.getByLabelText("Copy on select"))
    type("attention")
    expect(screen.queryByLabelText("Copy on select")).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    const [patch] = saveSettings.mock.calls[0]
    expect(patch.ui).toEqual({ copy_on_select: false })
  })
})

describe("CustomizeWebappDialog focus rings", () => {
  // A browser check must Tab down the settings and see every ring whole at
  // 1024px and at phone width, the rows still lined up with the search field.
  it("pads the scrolled content by the ring's width and takes the room back outward", () => {
    seed()
    render(<CustomizeWebappDialog />)
    const root = document.querySelector('[data-slot="scroll-area"]')!
    expect(root.className).toContain("-ml-[3px]")
    const content = document.querySelector('[data-slot="scroll-area-viewport"]')!
      .firstElementChild!
    expect(content.className).toContain("px-[3px]")
    expect(content.className).toContain("pb-[3px]")
  })
})

describe("CustomizeWebappDialog resets", () => {
  const save = () => fireEvent.click(screen.getByRole("button", { name: "Save" }))
  const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)))

  it("writes nothing when a reset is pressed, before anyone has saved", async () => {
    seed({ title: "prod dux", copy_on_select: false })
    render(<CustomizeWebappDialog />)
    topic("Changed from default")

    fireEvent.click(
      screen.getAllByRole("button", { name: /^Reset all \d+…$/ })[0],
    )

    await settle()
    expect(setInstanceIdentity).not.toHaveBeenCalled()
    expect(saveSettings).not.toHaveBeenCalled()
  })

  it("drafts a row's reset, shows it unsaved, and writes it only at Save", async () => {
    seed({ copy_on_select: false })
    render(<CustomizeWebappDialog />)
    await opened()
    topic("Typing and touch")
    const row = () => screen.getByLabelText("Copy on select").closest("[data-setting-row]")!
    expect(row().textContent).toContain("Changed from default:")

    fireEvent.click(screen.getByRole("button", { name: "Reset Copy on select to On" }))
    await settle()

    expect(screen.getByLabelText("Copy on select").getAttribute("aria-checked")).toBe("true")
    expect(row().textContent).not.toContain("Changed from default:")
    expect(screen.queryByRole("button", { name: "Reset Copy on select to On" })).toBeNull()
    expect(row().textContent).toContain("Not saved yet. Saved: Off")
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Put Copy on select back to its saved value, Off" }),
    )
    expect(screen.getByText("1 unsaved change")).toBeTruthy()
    expect(saveSettings).not.toHaveBeenCalled()

    save()
    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    expect(saveSettings.mock.calls[0][0]).toEqual({ ui: { copy_on_select: true } })
  })

  it("counts and marks what differs from the default, the draft included", () => {
    seed({ title: "", github_integration: true })
    render(<CustomizeWebappDialog />)
    const nav = screen.getByRole("navigation", { name: "Preferences topics" })
    const item = (name: string) =>
      within(nav).getByRole("button", { name: new RegExp(`^${name}`) }).textContent
    expect(item("Changed from default")).toContain("0, 0 settings")
    expect(item("Appearance")).not.toContain("some changed")

    fireEvent.change(screen.getByLabelText("Terminal font size"), { target: { value: "16" } })

    expect(item("Changed from default")).toContain("1, 1 setting")
    expect(item("Appearance")).toContain("some changed from default")
  })

  it("shows an empty Changed list when everything is at its default", () => {
    seed({ title: "", github_integration: true })
    render(<CustomizeWebappDialog />)
    topic("Changed from default")
    expect(screen.getByText("Everything is at its default")).toBeTruthy()
    expect(screen.queryByRole("button", { name: /^Reset all/ })).toBeNull()
  })

  it("asks before Reset all, and Cancel keeps every setting as it was", async () => {
    seed({ title: "prod dux", copy_on_select: false })
    render(<CustomizeWebappDialog />)
    await opened()
    topic("Changed from default")

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Reset all 3…" }))
    })
    const confirm = await screen.findByRole("dialog", {
      name: "Reset 3 settings to their defaults?",
    })
    expect(confirm.textContent).toContain(
      "Instance name, Copy on select and GitHub integration go back to what dux ships with.",
    )
    const cancel = within(confirm).getByRole("button", { name: "Cancel" })
    expect(document.activeElement).toBe(cancel)
    fireEvent.click(cancel)
    await settle()

    expect(screen.getByRole("button", { name: "Reset all 3…" })).toBeTruthy()
    expect(screen.queryByText(/unsaved change/)).toBeNull()
  })

  it("drafts every reset Reset all confirms, the bespoke and locked rows included, and Save writes each its own way", async () => {
    setTailscaleMode.mockResolvedValue({ mode: "auto", warning: true, message: "kept for next run" })
    seed({
      title: "prod dux",
      favicon: "amber",
      copy_on_select: false,
      show_changes_pane: false,
      github_integration: false,
      tailscale_mode: "no",
      tailscale_forced_no: true,
    })
    render(<CustomizeWebappDialog />)
    await opened()
    topic("Changed from default")

    fireEvent.click(screen.getByRole("button", { name: "Reset all 6…" }))
    const confirm = await screen.findByRole("dialog", {
      name: "Reset 6 settings to their defaults?",
    })
    fireEvent.click(within(confirm).getByRole("button", { name: "Reset 6 settings" }))
    await settle()

    expect(setInstanceIdentity).not.toHaveBeenCalled()
    expect(saveSettings).not.toHaveBeenCalled()
    expect(setChangesPaneVisibility).not.toHaveBeenCalled()
    expect(toggleGithubIntegration).not.toHaveBeenCalled()
    expect(setTailscaleMode).not.toHaveBeenCalled()
    expect(screen.getByText("6 unsaved changes")).toBeTruthy()
    // The rows stay listed until saved, so the reset can be undone where it was made.
    expect(screen.getByLabelText("Instance name")).toBeTruthy()
    await waitFor(() =>
      expect(document.activeElement).toBe(
        screen.getByRole("button", { name: /^Changed from default/ }),
      ),
    )

    save()
    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    // Identity fields go back to empty, which the server resolves to the shipped ones.
    expect(setInstanceIdentity).toHaveBeenCalledWith({ title: "", favicon: "" })
    expect(saveSettings.mock.calls[0][0]).toEqual({ ui: { copy_on_select: true } })
    expect(setChangesPaneVisibility).toHaveBeenCalledWith(true)
    expect(toggleGithubIntegration).toHaveBeenCalledTimes(1)
    expect(setTailscaleMode).toHaveBeenCalledWith("auto")
  })
})

describe("CustomizeWebappDialog closing", () => {
  it("asks before Cancel throws unsaved changes away, and Keep editing keeps them", async () => {
    seed()
    render(<CustomizeWebappDialog />)
    fireEvent.change(screen.getByLabelText("Instance name"), { target: { value: "renamed" } })

    await opened()
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    })
    const confirm = await screen.findByRole("dialog", { name: "Discard unsaved changes?" })
    expect(document.activeElement).toBe(within(confirm).getByRole("button", { name: "Keep editing" }))
    expect(confirm.textContent).toContain("1 unsaved change")
    expect(closeCustomizeWebapp).not.toHaveBeenCalled()

    fireEvent.click(within(confirm).getByRole("button", { name: "Keep editing" }))
    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: "Discard unsaved changes?" })).toBeNull(),
    )
    expect((screen.getByLabelText("Instance name") as HTMLInputElement).value).toBe("renamed")

    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    fireEvent.click(
      within(await screen.findByRole("dialog", { name: "Discard unsaved changes?" })).getByRole(
        "button",
        { name: "Discard changes" },
      ),
    )
    expect(closeCustomizeWebapp).toHaveBeenCalled()
    expect(setInstanceIdentity).not.toHaveBeenCalled()
  })

  it("asks on Escape too when there are unsaved changes", async () => {
    seed()
    render(<CustomizeWebappDialog />)
    fireEvent.change(screen.getByLabelText("Instance name"), { target: { value: "renamed" } })

    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" })
    expect(await screen.findByRole("dialog", { name: "Discard unsaved changes?" })).toBeTruthy()
    expect(closeCustomizeWebapp).not.toHaveBeenCalled()
  })

  it("sets unsaved changes aside on Back, which cannot stop to ask", async () => {
    seed()
    render(<CustomizeWebappDialog />)
    fireEvent.change(screen.getByLabelText("Instance name"), { target: { value: "renamed" } })
    // Moved and moved back: not a change.
    fireEvent.change(screen.getByLabelText("Terminal font size"), { target: { value: "16" } })
    fireEvent.change(screen.getByLabelText("Terminal font size"), { target: { value: "14" } })

    await back()
    expect(setAsidePreferencesDraft).toHaveBeenCalledWith(
      { values: { "server.title": "renamed" }, saved: { "server.title": "old instance" } },
      1,
      false,
    )
  })

  it("opens on the edits a Back set aside", () => {
    seed()
    mockState.customizeWebappDraft = {
      values: { "server.title": "renamed" },
      saved: { "server.title": "old instance" },
    }
    render(<CustomizeWebappDialog />)
    expect((screen.getByLabelText("Instance name") as HTMLInputElement).value).toBe("renamed")
    expect(screen.getByText("1 unsaved change")).toBeTruthy()
  })

  it("opens config.toml in its editor, and only when nothing is unsaved", () => {
    seed()
    render(<CustomizeWebappDialog />)
    fireEvent.change(screen.getByLabelText("Instance name"), { target: { value: "renamed" } })
    const open = screen.getByRole("button", { name: "Open config.toml…" }) as HTMLButtonElement
    expect(open.disabled).toBe(true)

    fireEvent.change(screen.getByLabelText("Instance name"), { target: { value: "old instance" } })
    fireEvent.click(screen.getByRole("button", { name: "Open config.toml…" }))
    expect(closeCustomizeWebapp).toHaveBeenCalled()
    expect(openConfigEditor).toHaveBeenCalled()
  })
})

describe("CustomizeWebappDialog on a phone", () => {
  it("shows the topics first and a topic's settings as a second step, which Back leaves", async () => {
    phone = true
    seed()
    render(<CustomizeWebappDialog />)
    expect(screen.queryByLabelText("Instance name")).toBeNull()

    topic("Appearance")
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole("button", { name: "Back to all topics" })),
    )
    fireEvent.change(screen.getByLabelText("Instance name"), { target: { value: "renamed" } })
    expect(screen.getByRole("button", { name: "Save 1 change" })).toBeTruthy()

    await back()
    expect(screen.queryByLabelText("Instance name")).toBeNull()
    expect(screen.getByRole("navigation", { name: "Preferences topics" })).toBeTruthy()
    expect(setAsidePreferencesDraft).not.toHaveBeenCalled()
    expect(closeCustomizeWebapp).not.toHaveBeenCalled()

    await back()
    expect(setAsidePreferencesDraft).toHaveBeenCalledWith(
      { values: { "server.title": "renamed" }, saved: { "server.title": "old instance" } },
      1,
      false,
    )
  })

  it("opens straight on the topic an opener names", () => {
    phone = true
    seed()
    mockState.customizeWebappTopic = "network"
    render(<CustomizeWebappDialog />)
    expect(screen.getByLabelText("Bind your Tailscale address")).toBeTruthy()
  })

  // The topics' sheet takes its entry first, so Back from a topic opened
  // straight away still stops on the topics.
  it("Back goes to the topics first when it opened straight on a topic", async () => {
    phone = true
    seed()
    mockState.customizeWebappTopic = "network"
    render(<CustomizeWebappDialog />)
    await opened()
    expect(openModals()).toHaveLength(2)

    await back()
    expect(screen.queryByLabelText("Bind your Tailscale address")).toBeNull()
    expect(screen.getByRole("navigation", { name: "Preferences topics" })).toBeTruthy()
    expect(closeCustomizeWebapp).not.toHaveBeenCalled()

    await back()
    expect(closeCustomizeWebapp).toHaveBeenCalled()
  })
})

describe("CustomizeWebappDialog unsaved rows", () => {
  const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)))
  const rowOf = (label: string) => screen.getByLabelText(label).closest("[data-setting-row]")!

  it("says what is saved on a reset row and puts it back where the reset was made", async () => {
    seed({ title: "prod dux" })
    render(<CustomizeWebappDialog />)
    await opened()

    fireEvent.click(screen.getByRole("button", { name: "Reset Instance name to dux" }))
    await settle()
    expect(rowOf("Instance name").textContent).toContain("Not saved yet. Saved: prod dux")
    const undo = screen.getByRole("button", {
      name: "Put Instance name back to its saved value, prod dux",
    })
    expect(document.activeElement).toBe(undo)

    fireEvent.click(undo)
    await settle()
    expect((screen.getByLabelText("Instance name") as HTMLInputElement).value).toBe("prod dux")
    expect(rowOf("Instance name").textContent).not.toContain("Not saved yet")
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Reset Instance name to dux" }),
    )
    expect(screen.queryByText(/unsaved change/)).toBeNull()
  })

  it("offers the saved value back after an ordinary edit too", async () => {
    seed()
    render(<CustomizeWebappDialog />)
    fireEvent.change(screen.getByLabelText("Terminal font size"), { target: { value: "16" } })
    expect(rowOf("Terminal font size").textContent).toContain("Not saved yet. Saved: 14 px")

    fireEvent.click(
      screen.getByRole("button", { name: "Put Terminal font size back to its saved value, 14 px" }),
    )
    expect((screen.getByLabelText("Terminal font size") as HTMLInputElement).value).toBe("14")
    expect(screen.queryByText(/unsaved change/)).toBeNull()
  })

  it("keeps a reset row in Changed from default, with focus on it", async () => {
    seed({ title: "", github_integration: true })
    render(<CustomizeWebappDialog />)
    await opened()
    fireEvent.change(screen.getByLabelText("Terminal font size"), { target: { value: "16" } })
    topic("Changed from default")

    fireEvent.click(screen.getByRole("button", { name: "Reset Terminal font size to 14 px" }))
    await settle()

    expect(screen.getByLabelText("Terminal font size")).toBeTruthy()
    expect(document.activeElement).toBe(screen.getByLabelText("Terminal font size"))
  })

  it("keeps a search result whose matching sentence an edit took away", async () => {
    vi.stubGlobal("Notification", { permission: "default", requestPermission: vi.fn() })
    seed()
    render(<CustomizeWebappDialog />)
    fireEvent.change(screen.getByLabelText("Search settings"), { target: { value: "grant" } })
    const toggle = screen.getByLabelText("Desktop notifications")

    fireEvent.click(toggle)

    expect(screen.getByLabelText("Desktop notifications")).toBeTruthy()
  })

  it("says on a restored edit that the saved value has moved since the edit was made", () => {
    seed({ terminal_font_size: 18 })
    mockState.customizeWebappDraft = {
      values: { "ui.terminal_font_size": 16 },
      saved: { "ui.terminal_font_size": 14 },
    }
    render(<CustomizeWebappDialog />)
    expect(rowOf("Terminal font size").textContent).toContain(
      "The saved value changed to 18 px since this edit was made.",
    )
  })

  it("says so when the server changes a row while it holds an unsaved edit", () => {
    seed()
    const { rerender } = render(<CustomizeWebappDialog />)
    fireEvent.change(screen.getByLabelText("Terminal font size"), { target: { value: "16" } })
    expect(rowOf("Terminal font size").textContent).not.toContain("changed to")

    seed({ terminal_font_size: 20 })
    rerender(<CustomizeWebappDialog />)
    expect(rowOf("Terminal font size").textContent).toContain(
      "The saved value changed to 20 px since this edit was made.",
    )
  })

  it("after a partial save says what was saved, and a retry sends only the rest", async () => {
    seed({ github_integration: false, tailscale_mode: "auto" })
    setTailscaleMode.mockRejectedValueOnce(new Error("refused"))
    render(<CustomizeWebappDialog />)
    fireEvent.change(field("Terminal font size"), { target: { value: "16" } })
    fireEvent.click(field("GitHub integration"))
    const trigger = field("Bind your Tailscale address")
    fireEvent.click(trigger)
    await waitFor(() => expect(trigger.getAttribute("aria-expanded")).toBe("true"))
    const option = await screen.findByRole("option", { name: "No" })
    fireEvent.pointerDown(option, { pointerType: "mouse" })
    fireEvent.click(option)

    fireEvent.click(screen.getByRole("button", { name: "Save" }))
    const notice = await screen.findByText(/^Part of the save went through/)
    expect(notice.textContent).toBe(
      "Part of the save went through. Saved: Terminal font size and GitHub integration. Not saved: Bind your Tailscale address, still marked unsaved; Save tries it again.",
    )
    expect(screen.getByText("1 unsaved change")).toBeTruthy()
    expect(closeCustomizeWebapp).not.toHaveBeenCalled()

    fireEvent.click(screen.getByRole("button", { name: "Save" }))
    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    expect(toggleGithubIntegration).toHaveBeenCalledTimes(1)
    expect(saveSettings).toHaveBeenCalledTimes(1)
    expect(setTailscaleMode).toHaveBeenCalledTimes(2)
  })
})

describe("CustomizeWebappDialog search scope", () => {
  const type = (query: string) =>
    fireEvent.change(screen.getByLabelText("Search settings"), { target: { value: query } })

  it("does not match where a row applies, nor the default it names", () => {
    seed({ title: "prod dux" })
    render(<CustomizeWebappDialog />)
    type("browser")
    expect(screen.queryByLabelText("Terminal font size")).toBeNull()
    type("app")
    expect(screen.queryByLabelText("Attention grace")).toBeNull()
    type("default")
    expect(screen.queryByLabelText("Instance name")).toBeNull()
  })

  it("matches option labels and topic names", () => {
    seed()
    render(<CustomizeWebappDialog />)
    type("top")
    expect(screen.getByLabelText("PR banner position")).toBeTruthy()
    type("sign-in network")
    expect(screen.getByLabelText("Bind your Tailscale address")).toBeTruthy()
  })
})

describe("CustomizeWebappDialog edge rows", () => {
  const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)))

  it("treats a long-paste row an older server does not publish as unchanged", () => {
    seed({ title: "", github_integration: true, upload_pasted_text_chars: undefined })
    render(<CustomizeWebappDialog />)
    const nav = screen.getByRole("navigation", { name: "Preferences topics" })
    expect(
      within(nav).getByRole("button", { name: /^Changed from default/ }).textContent,
    ).toContain("0, 0 settings")
  })

  it("Reset all resets exactly the settings its confirm named", async () => {
    seed({ title: "prod dux", github_integration: true })
    const { rerender } = render(<CustomizeWebappDialog />)
    await opened()
    topic("Changed from default")
    fireEvent.click(screen.getByRole("button", { name: "Reset all 1…" }))
    const confirm = await screen.findByRole("dialog", { name: "Reset 1 setting to its default?" })

    seed({ title: "prod dux", github_integration: true, copy_on_select: false })
    rerender(<CustomizeWebappDialog />)
    fireEvent.click(within(confirm).getByRole("button", { name: "Reset 1 setting" }))
    await settle()

    fireEvent.click(screen.getByRole("button", { name: "Save" }))
    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    expect(setInstanceIdentity).toHaveBeenCalledWith({ title: "" })
    expect(saveSettings).not.toHaveBeenCalled()
  })
})
