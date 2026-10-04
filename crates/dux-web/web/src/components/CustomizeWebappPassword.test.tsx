// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import type { ReactNode } from "react"

import type { AuthStatus } from "@/lib/authApi"
import type { AuthPhase } from "@/lib/authGate"
import type { Bootstrap } from "@/lib/bootstrapApi"
import type { DuxState } from "@/lib/store"
import type { Strength } from "@/lib/passwordStrength"

// The Preferences password row: its own write target, the strength meter, the
// minimums, the first-password rule, and what happens to this page after a
// change signs every browser out.

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
vi.mock("@/lib/configApi", () => ({
  configApi: { toggleGithubIntegration: vi.fn(), setTailscaleMode: vi.fn() },
}))
vi.mock("@/lib/notify", () => ({
  notifyError: vi.fn(),
  notifyInfo: vi.fn(),
  notifySuccess: vi.fn(),
  notifyWarning: vi.fn(),
}))
vi.mock("@/components/SimpleTooltip", () => ({
  SimpleTooltip: ({ children }: { children: ReactNode }) => <>{children}</>,
}))

let phase: AuthPhase
const afterPasswordChange = vi.fn(async () => {})
const reportUnauthorized = vi.fn()
const refreshAuthStatus = vi.fn(async () => {})
vi.mock("@/lib/authGate", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/authGate")>()
  return {
    ...actual,
    useAuthPhase: () => phase,
    afterPasswordChange,
    reportUnauthorized,
    refreshAuthStatus,
  }
})
const postPassword = vi.fn()
vi.mock("@/lib/authActions", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/authActions")>()
  return { ...actual, postPassword }
})
// How many meter loads fail before one works.
let strengthFailures = 0
// A deterministic meter: the score is the number of spaces, capped at 4.
vi.mock("@/lib/passwordStrength", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/passwordStrength")>()
  return {
    ...actual,
    estimateStrength: async (pw: string): Promise<Strength> => {
      if (strengthFailures > 0) {
        strengthFailures--
        throw new Error("chunk failed to load")
      }
      const score = Math.min(4, pw.split(" ").length - 1) as Strength["score"]
      return {
        score,
        label: actual.STRENGTH_LABELS[score],
        hint: score < 2 ? "Add another word or two." : null,
      }
    },
  }
})

const mem = new Map<string, string>()
vi.stubGlobal("localStorage", {
  getItem: (k: string) => mem.get(k) ?? null,
  setItem: (k: string, v: string) => void mem.set(k, String(v)),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})
vi.stubGlobal("fetch", vi.fn(() => Promise.reject(new Error("offline test"))))

const { CustomizeWebappDialog } = await import("./CustomizeWebappDialog")
const store = await import("@/lib/store")
const closeCustomizeWebapp = vi.mocked(store.closeCustomizeWebapp)
const saveSettings = vi.mocked(store.saveSettings)
const notify = await import("@/lib/notify")
const notifySuccess = vi.mocked(notify.notifySuccess)

function status(overrides: Partial<AuthStatus> = {}): AuthStatus {
  return {
    password_set: true,
    required_here: true,
    signed_in: true,
    client_class: "network",
    transport_encrypted: true,
    no_auth_warning: false,
    weak_password: false,
    can_set_first_password: false,
    auth_broken: null,
    minimum_password_length: 12,
    minimum_password_score: 2,
    ...overrides,
  }
}

function seed() {
  mockState = {
    customizeWebappOpen: true,
    changesPaneOverride: null,
    bootstrap: {
      available_providers: [],
      macros: [],
      welcome_tips: [],
      dux_version: "v0.0.0",
      upload_pasted_text_chars: 4000,
    } as unknown as Bootstrap,
  } as unknown as DuxState
}

const STRONG = "glacier mango typewriter lantern"

beforeEach(() => {
  seed()
  phase = { kind: "open", status: status() }
  postPassword.mockReset().mockResolvedValue({ kind: "ok" })
  strengthFailures = 0
  refreshAuthStatus.mockClear()
  afterPasswordChange.mockClear()
  reportUnauthorized.mockClear()
  closeCustomizeWebapp.mockClear()
  saveSettings.mockClear().mockResolvedValue(true)
  notifySuccess.mockClear()
})

afterEach(() => {
  cleanup()
})

function type(label: string, value: string) {
  fireEvent.change(screen.getByLabelText(label), { target: { value } })
}

async function save() {
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: "Save" }))
  })
}

describe("the password row", () => {
  it("asks for the current password and the new one twice when a password is set", () => {
    render(<CustomizeWebappDialog />)
    expect(screen.getByLabelText("Current password")).toHaveProperty("type", "password")
    expect(screen.getByLabelText("New password")).toHaveProperty("type", "password")
    expect(screen.getByLabelText("New password again")).toHaveProperty("type", "password")
  })

  it("has no reset-to-defaults for its section", () => {
    render(<CustomizeWebappDialog />)
    expect(screen.getAllByRole("button", { name: "Reset section to defaults…" })).toHaveLength(3)
  })

  it("shows the strength live, from weak to excellent, with the hint", async () => {
    render(<CustomizeWebappDialog />)
    type("New password", "correcthorse")
    const meter = await screen.findByRole("meter")
    await waitFor(() => expect(meter.getAttribute("aria-valuetext")).toBe("Weak"))
    expect(screen.getByText("Add another word or two.")).toBeTruthy()
    type("New password", STRONG)
    await waitFor(() =>
      expect(screen.getByRole("meter").getAttribute("aria-valuetext")).toBe("Strong"),
    )
  })

  it("changes the password with the current one, then asks where that leaves this page", async () => {
    render(<CustomizeWebappDialog />)
    type("Current password", "the old password")
    type("New password", STRONG)
    type("New password again", STRONG)
    await waitFor(() => expect(screen.getByRole("meter").getAttribute("aria-valuetext")).toBe("Strong"))
    await save()
    expect(postPassword).toHaveBeenCalledWith({ current: "the old password", next: STRONG })
    expect(afterPasswordChange).toHaveBeenCalled()
    expect(closeCustomizeWebapp).toHaveBeenCalled()
    // Never through the generic PATCH.
    expect(saveSettings).not.toHaveBeenCalled()
  })

  it("refuses below the minimum length without sending, and says so under the row", async () => {
    render(<CustomizeWebappDialog />)
    type("Current password", "the old password")
    type("New password", "a b c")
    type("New password again", "a b c")
    await save()
    expect(postPassword).not.toHaveBeenCalled()
    expect(screen.getByRole("alert").textContent).toBe("Use at least 12 characters.")
    expect(closeCustomizeWebapp).not.toHaveBeenCalled()
  })

  it("shows a low score but sends anyway, because the server decides", async () => {
    render(<CustomizeWebappDialog />)
    type("Current password", "the old password")
    type("New password", "correcthorsebattery")
    type("New password again", "correcthorsebattery")
    await waitFor(() => expect(screen.getByRole("meter").getAttribute("aria-valuetext")).toBe("Weak"))
    await save()
    expect(postPassword).toHaveBeenCalledWith({
      current: "the old password",
      next: "correcthorsebattery",
    })
  })

  it("shows the server's refusal of a weak password under the row", async () => {
    postPassword.mockResolvedValue({
      kind: "refused",
      message: "That password is too easy to guess, so dux refused it.",
      score: 1,
    })
    render(<CustomizeWebappDialog />)
    type("Current password", "the old password")
    type("New password", "correcthorsebattery")
    type("New password again", "correcthorsebattery")
    await save()
    expect(screen.getByRole("alert").textContent).toContain("too easy to guess")
  })

  it("does not lock Save when the meter cannot load, and offers to load it again", async () => {
    strengthFailures = 1
    // Refused, so the fields stay and the meter can be asked for again after.
    postPassword.mockResolvedValue({ kind: "refused", message: "no", score: null })
    render(<CustomizeWebappDialog />)
    type("Current password", "the old password")
    type("New password", STRONG)
    type("New password again", STRONG)
    const retry = await screen.findByRole("button", { name: "Load the strength meter again" })
    await save()
    expect(postPassword).toHaveBeenCalledWith({ current: "the old password", next: STRONG })
    await act(async () => fireEvent.click(retry))
    await waitFor(() =>
      expect(screen.getByRole("meter").getAttribute("aria-valuetext")).toBe("Strong"),
    )
  })

  it("reads the sign-in status again every time Preferences opens", () => {
    render(<CustomizeWebappDialog />)
    expect(refreshAuthStatus).toHaveBeenCalledTimes(1)
  })

  it("refuses a mismatched repeat", async () => {
    render(<CustomizeWebappDialog />)
    type("Current password", "the old password")
    type("New password", STRONG)
    type("New password again", `${STRONG}x`)
    await waitFor(() => expect(screen.getByRole("meter").getAttribute("aria-valuetext")).toBe("Strong"))
    await save()
    expect(postPassword).not.toHaveBeenCalled()
    expect(screen.getByRole("alert").textContent).toBe("The two new passwords do not match.")
  })

  it("saves nothing at all when the password fields are invalid", async () => {
    mockState = {
      ...mockState,
      bootstrap: { ...mockState.bootstrap!, copy_on_select: true },
    } as DuxState
    render(<CustomizeWebappDialog />)
    fireEvent.click(screen.getByRole("switch", { name: "Copy on select" }))
    type("New password", "short")
    await save()
    expect(saveSettings).not.toHaveBeenCalled()
  })

  it("shows the server's refusal and stays open", async () => {
    postPassword.mockResolvedValue({
      kind: "refused",
      message: "The current password is not right.",
      score: null,
    })
    render(<CustomizeWebappDialog />)
    type("Current password", "wrong")
    type("New password", STRONG)
    type("New password again", STRONG)
    await waitFor(() => expect(screen.getByRole("meter").getAttribute("aria-valuetext")).toBe("Strong"))
    await save()
    expect(screen.getByRole("alert").textContent).toBe("The current password is not right.")
    expect(closeCustomizeWebapp).not.toHaveBeenCalled()
  })

  it("hands a signed-out answer to the gate", async () => {
    postPassword.mockResolvedValue({ kind: "signed_out" })
    render(<CustomizeWebappDialog />)
    type("Current password", "x")
    type("New password", STRONG)
    type("New password again", STRONG)
    await waitFor(() => expect(screen.getByRole("meter").getAttribute("aria-valuetext")).toBe("Strong"))
    await save()
    expect(reportUnauthorized).toHaveBeenCalled()
  })

  it("sets a first password where that is allowed, with no current-password field", async () => {
    phase = {
      kind: "open",
      status: status({
        password_set: false,
        required_here: false,
        signed_in: false,
        can_set_first_password: true,
      }),
    }
    render(<CustomizeWebappDialog />)
    expect(screen.queryByLabelText("Current password")).toBeNull()
    type("New password", STRONG)
    type("New password again", STRONG)
    await waitFor(() => expect(screen.getByRole("meter").getAttribute("aria-valuetext")).toBe("Strong"))
    await save()
    expect(postPassword).toHaveBeenCalledWith({ next: STRONG })
    expect(notifySuccess).toHaveBeenCalled()
  })

  it("says where the first password can be set when it cannot be set from here", () => {
    phase = {
      kind: "open",
      status: status({ password_set: false, required_here: false, signed_in: false }),
    }
    render(<CustomizeWebappDialog />)
    expect(screen.queryByLabelText("New password")).toBeNull()
    expect(document.body.textContent).toContain("dux config set server.auth.password")
  })

  it("says it cannot be changed here when the sign-in status is unknown, and asks again", () => {
    phase = { kind: "open", status: null }
    render(<CustomizeWebappDialog />)
    expect(screen.queryByLabelText("New password")).toBeNull()
    // Reopening really does ask again: opening is what reads the status.
    expect(refreshAuthStatus).toHaveBeenCalled()
  })

  it("keeps the password fields at the touch floor", () => {
    render(<CustomizeWebappDialog />)
    for (const label of ["Current password", "New password", "New password again"]) {
      expect(screen.getByLabelText(label).className).toContain("pointer-coarse:min-h-11")
    }
  })
})
