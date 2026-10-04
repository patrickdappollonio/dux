// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { AuthStatus } from "@/lib/authApi"
import type { AuthPhase } from "@/lib/authGate"

let phase: AuthPhase = { kind: "checking" }
const noteAuthStatus = vi.fn()
vi.mock("@/lib/authGate", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/authGate")>()
  return { ...actual, useAuthPhase: () => phase, noteAuthStatus }
})
const postDismissNoAuthWarning = vi.fn(async () => {})
vi.mock("@/lib/authApi", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/authApi")>()
  return { ...actual, postDismissNoAuthWarning }
})
const openCustomizeWebapp = vi.fn()
vi.mock("@/lib/store", () => ({ openCustomizeWebapp }))
const notifySuccess = vi.fn()
const notifyError = vi.fn()
vi.mock("@/lib/notify", () => ({ notifySuccess, notifyError }))

const { AuthGate } = await import("./AuthGate")
const { resetBannerDismissals } = await import("@/lib/bannerDismissals")

function status(overrides: Partial<AuthStatus> = {}): AuthStatus {
  return {
    password_set: false,
    required_here: false,
    signed_in: false,
    client_class: "network",
    transport_encrypted: false,
    no_auth_warning: false,
    weak_password: false,
    can_set_first_password: false,
    auth_broken: null,
    minimum_password_length: null,
    minimum_password_score: null,
    ...overrides,
  }
}

beforeEach(() => {
  resetBannerDismissals()
})

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

function renderGate() {
  return render(
    <AuthGate>
      <div>the app</div>
    </AuthGate>,
  )
}

describe("AuthGate", () => {
  it("renders neither the app nor a form while the first answer is coming", () => {
    phase = { kind: "checking" }
    renderGate()
    expect(screen.queryByText("the app")).toBeNull()
    expect(screen.queryByLabelText("Password")).toBeNull()
  })

  it("renders the app when open", () => {
    phase = { kind: "open", status: null }
    renderGate()
    expect(screen.getByText("the app")).toBeTruthy()
  })

  it("renders the login page instead of the app when signed out", () => {
    phase = { kind: "signed_out", status: status(), reason: "expired" }
    renderGate()
    expect(screen.queryByText("the app")).toBeNull()
    expect(screen.getByLabelText("Password")).toBeTruthy()
  })

  it("renders the blocked page", () => {
    phase = { kind: "blocked", where: "config.toml" }
    renderGate()
    expect(screen.getByRole("heading", { name: "This address is blocked" })).toBeTruthy()
  })

  it("renders the broken page", () => {
    phase = { kind: "broken", detail: "bad hash" }
    renderGate()
    expect(screen.getByRole("heading", { name: "Sign-in is misconfigured" })).toBeTruthy()
  })
})

describe("the no-password banner", () => {
  it("is red, says what anyone can do, and how to set a password", () => {
    phase = {
      kind: "open",
      status: status({ no_auth_warning: true, can_set_first_password: true }),
    }
    renderGate()
    const banner = screen.getByRole("alert")
    expect(banner.className).toContain("bg-destructive")
    expect(banner.textContent).toContain("No password")
    expect(banner.textContent).toContain("anyone")
    expect(banner.textContent).toContain("Preferences")
    expect(banner.textContent).toContain("dux config set server.auth.password")
  })

  it("does not offer Preferences where the first password cannot be set from here", () => {
    phase = { kind: "open", status: status({ no_auth_warning: true }) }
    renderGate()
    expect(screen.getByRole("alert").textContent).not.toContain("Preferences")
  })

  it("Dismiss hides it for this page only, writing nothing", async () => {
    phase = { kind: "open", status: status({ no_auth_warning: true }) }
    renderGate()
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Dismiss" })))
    expect(screen.queryByRole("alert")).toBeNull()
    expect(postDismissNoAuthWarning).not.toHaveBeenCalled()
  })

  it("Don't show again writes the setting, hides it and says where it went", async () => {
    phase = { kind: "open", status: status({ no_auth_warning: true }) }
    renderGate()
    await act(async () =>
      fireEvent.click(screen.getByRole("button", { name: "Don't show again" })),
    )
    expect(postDismissNoAuthWarning).toHaveBeenCalledTimes(1)
    expect(noteAuthStatus).toHaveBeenCalledWith(
      expect.objectContaining({ no_auth_warning: false }),
    )
    expect(notifySuccess).toHaveBeenCalledTimes(1)
    expect(JSON.stringify(notifySuccess.mock.calls[0][0])).toContain(
      "disable_no_auth_warning",
    )
  })

  it("keeps the banner and says why when the write fails", async () => {
    postDismissNoAuthWarning.mockRejectedValueOnce(new Error("could not write config.toml"))
    phase = { kind: "open", status: status({ no_auth_warning: true }) }
    renderGate()
    await act(async () =>
      fireEvent.click(screen.getByRole("button", { name: "Don't show again" })),
    )
    expect(notifyError).toHaveBeenCalledWith("could not write config.toml")
    expect(screen.getByRole("alert")).toBeTruthy()
  })

  it("is absent when the server does not ask for it", () => {
    phase = { kind: "open", status: status() }
    renderGate()
    expect(screen.queryByRole("alert")).toBeNull()
  })
})

describe("the weak-password banner", () => {
  it("points to Preferences", async () => {
    phase = {
      kind: "open",
      status: status({ password_set: true, signed_in: true, weak_password: true }),
    }
    renderGate()
    const banner = screen.getByRole("status")
    expect(banner.textContent).toContain("weaker than")
    await act(async () =>
      fireEvent.click(screen.getByRole("button", { name: "Change it in Preferences…" })),
    )
    expect(openCustomizeWebapp).toHaveBeenCalled()
  })

  it("can be dismissed for this page", async () => {
    phase = {
      kind: "open",
      status: status({ password_set: true, signed_in: true, weak_password: true }),
    }
    renderGate()
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Dismiss" })))
    expect(screen.queryByRole("status")).toBeNull()
  })
})

describe("the hooks the browser journeys hold on to", () => {
  it("marks both banners and the Don't show again control", () => {
    phase = {
      kind: "open",
      status: status({ no_auth_warning: true, weak_password: true }),
    }
    renderGate()
    expect(screen.getByTestId("no-auth-banner").getAttribute("role")).toBe("alert")
    expect(screen.getByTestId("no-auth-banner-never").textContent).toBe("Don't show again")
    expect(screen.getByTestId("weak-password-banner").getAttribute("role")).toBe("status")
  })
})
