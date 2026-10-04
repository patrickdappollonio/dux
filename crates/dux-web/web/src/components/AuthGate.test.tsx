// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { AuthStatus } from "@/lib/authApi"
import type { AuthPhase } from "@/lib/authGate"

let phase: AuthPhase = { kind: "checking" }
const refreshAuthStatus = vi.fn(async () => {})
const retryAuthGate = vi.fn(async () => {})
vi.mock("@/lib/authGate", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/authGate")>()
  return { ...actual, useAuthPhase: () => phase, refreshAuthStatus, retryAuthGate }
})
const postDismissNoAuthWarning = vi.fn(async () => {})
vi.mock("@/lib/authActions", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/authActions")>()
  return { ...actual, postDismissNoAuthWarning }
})
// The toaster is the real one's stand-in: what matters is where it is mounted.
vi.mock("@/components/ui/sonner", () => ({
  Toaster: () => <section data-testid="toaster" />,
}))
const openCustomizeWebapp = vi.fn()
vi.mock("@/lib/store", () => ({ openCustomizeWebapp }))
const notifySuccess = vi.fn()
const notifyError = vi.fn()
vi.mock("@/lib/notify", () => ({ notifySuccess, notifyError }))

const { AuthGate } = await import("./AuthGate")
const { Dialog, DialogContent, DialogTitle } = await import("@/components/ui/dialog")
const { useDividerDrag } = await import("@/hooks/use-divider-drag")
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

  it("never leaves the first look blank: it says it is connecting after a moment", async () => {
    vi.useFakeTimers()
    phase = { kind: "checking" }
    renderGate()
    expect(screen.queryByText(/Connecting to dux/)).toBeNull()
    await act(async () => {
      vi.advanceTimersByTime(1500)
    })
    expect(screen.getByText(/Connecting to dux/)).toBeTruthy()
    vi.useRealTimers()
  })

  it("says it cannot reach dux, with a Retry, when the first look got no answer", async () => {
    phase = { kind: "unreachable", timedOut: true }
    renderGate()
    expect(screen.getByRole("heading", { name: "Can't reach dux" })).toBeTruthy()
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Retry" })))
    expect(retryAuthGate).toHaveBeenCalled()
  })

  it("stops on an explicit page when the server keeps refusing a session it calls valid", async () => {
    phase = { kind: "stuck" }
    renderGate()
    expect(
      screen.getByRole("heading", { name: "dux keeps refusing this browser" }),
    ).toBeTruthy()
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Try again" })))
    expect(retryAuthGate).toHaveBeenCalled()
  })

  it("keeps the toaster mounted at the root in every phase", () => {
    for (const p of [
      { kind: "checking" },
      { kind: "open", status: null },
      { kind: "signed_out", status: status(), reason: "expired" },
    ] as AuthPhase[]) {
      phase = p
      renderGate()
      expect(screen.getAllByTestId("toaster")).toHaveLength(1)
      cleanup()
    }
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
    // Hidden at once, and the status read again rather than patched by hand.
    expect(screen.queryByTestId("no-auth-banner")).toBeNull()
    expect(refreshAuthStatus).toHaveBeenCalled()
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

describe("signing out of a page that was in use", () => {
  it("keeps the app mounted under the login page, hidden and inert", () => {
    phase = { kind: "open", status: null }
    const { rerender } = renderGate()
    const app = screen.getByText("the app")
    phase = { kind: "signed_out", status: status(), reason: "expired" }
    rerender(
      <AuthGate>
        <div>the app</div>
      </AuthGate>,
    )
    // The same node: nothing remounted, so editors, drafts and dialogs survive.
    expect(screen.getByText("the app")).toBe(app)
    const shell = screen.getByTestId("app-shell")
    expect(shell.hasAttribute("inert")).toBe(true)
    expect(shell.style.visibility).toBe("hidden")
    expect(shell.getAttribute("aria-hidden")).toBe("true")
    expect(screen.getByLabelText("Password")).toBeTruthy()
  })

  it("comes back visible and live on signing in again", () => {
    phase = { kind: "open", status: null }
    const { rerender } = renderGate()
    phase = { kind: "signed_out", status: status(), reason: "expired" }
    rerender(<AuthGate><div>the app</div></AuthGate>)
    phase = { kind: "open", status: null }
    rerender(<AuthGate><div>the app</div></AuthGate>)
    const shell = screen.getByTestId("app-shell")
    expect(shell.hasAttribute("inert")).toBe(false)
    expect(shell.style.visibility).toBe("")
  })

  it("makes everything portalled outside the app inert too, and gives it back", () => {
    const portal = document.createElement("div")
    portal.id = "a-dialog-portal"
    document.body.appendChild(portal)
    phase = { kind: "open", status: null }
    const { rerender } = renderGate()
    phase = { kind: "signed_out", status: status(), reason: "expired" }
    rerender(<AuthGate><div>the app</div></AuthGate>)
    expect(portal.hasAttribute("inert")).toBe(true)
    phase = { kind: "open", status: null }
    rerender(<AuthGate><div>the app</div></AuthGate>)
    expect(portal.hasAttribute("inert")).toBe(false)
    portal.remove()
  })

  it("never mounts the app for a page that was signed out from the start", () => {
    phase = { kind: "signed_out", status: status(), reason: "required" }
    renderGate()
    expect(screen.queryByText("the app")).toBeNull()
  })
})

describe("banners in the layout", () => {
  it("sit in flow above the app, not over it", () => {
    phase = { kind: "open", status: status({ no_auth_warning: true }) }
    renderGate()
    const stack = screen.getByTestId("auth-banners")
    expect(stack.className).not.toContain("fixed")
    const app = screen.getByText("the app")
    expect(stack.compareDocumentPosition(app) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })

  it("read the status again when they first show", () => {
    phase = { kind: "open", status: status({ no_auth_warning: true }) }
    renderGate()
    expect(refreshAuthStatus).toHaveBeenCalled()
  })

  it("keep their buttons at the touch floor on a coarse pointer", () => {
    phase = { kind: "open", status: status({ no_auth_warning: true, weak_password: true }) }
    renderGate()
    for (const b of screen.getAllByRole("button")) {
      expect(b.className).toContain("pointer-coarse:min-h-11")
    }
  })
})

// Sign the page out of a session that was in use, with `app` mounted.
async function signOutOver(app: React.ReactNode) {
  phase = { kind: "open", status: status() }
  const r = render(<AuthGate>{app}</AuthGate>)
  await act(async () => {
    await new Promise((res) => setTimeout(res, 20))
  })
  phase = {
    kind: "signed_out",
    status: status({ password_set: true, required_here: true }),
    reason: "expired",
  }
  r.rerender(<AuthGate>{app}</AuthGate>)
  await act(async () => {
    await new Promise((res) => setTimeout(res, 20))
  })
  return r
}

describe("the gate layer", () => {
  it("lives in its own body-level layer, outside the app's root", async () => {
    const r = await signOutOver(<div>the app</div>)
    const field = screen.getByLabelText("Password")
    expect(r.container.contains(field)).toBe(false)
    const layer = field.closest("[data-auth-gate-layer]") as HTMLElement
    expect(layer.parentElement).toBe(document.body)
  })

  it("is a fixed, scrollable layer on a first load too", () => {
    phase = { kind: "signed_out", status: status(), reason: "required" }
    renderGate()
    const layer = screen.getByLabelText("Password").closest("[data-auth-gate-layer]")
    expect(layer?.firstElementChild?.className).toContain("fixed")
    expect(layer?.firstElementChild?.className).toContain("overflow-y-auto")
    // The layer is in the document before the field renders, so it gets focus.
    expect(document.activeElement).toBe(screen.getByLabelText("Password"))
  })

  it("stays audible with a modal dialog open in the hidden app", async () => {
    await signOutOver(
      <Dialog open>
        <DialogContent>
          <DialogTitle>Commit</DialogTitle>
          <input aria-label="dialog field" />
        </DialogContent>
      </Dialog>,
    )
    const field = screen.getByLabelText("Password")
    for (let el: Element | null = field; el; el = el.parentElement) {
      expect(el.getAttribute("aria-hidden"), el.tagName).not.toBe("true")
      expect(el.hasAttribute("inert"), el.tagName).toBe(false)
    }
  })

  it("does not let a press or Escape on the login page dismiss a hidden dialog", async () => {
    const onOpenChange = vi.fn()
    await signOutOver(
      <Dialog open onOpenChange={onOpenChange}>
        <DialogContent>
          <DialogTitle>Commit</DialogTitle>
          <textarea aria-label="draft" defaultValue="a long draft" />
        </DialogContent>
      </Dialog>,
    )
    const field = screen.getByLabelText("Password")
    field.focus()
    fireEvent.pointerDown(field, { pointerType: "mouse", button: 0 })
    fireEvent.mouseDown(field)
    fireEvent.pointerUp(field, { pointerType: "mouse" })
    fireEvent.mouseUp(field)
    fireEvent.click(field)
    fireEvent.keyDown(field, { key: "Escape" })
    await act(async () => {
      await new Promise((res) => setTimeout(res, 20))
    })
    expect(onOpenChange).not.toHaveBeenCalled()
  })

  it("keeps keys typed on the login page away from the app's own shortcuts", async () => {
    // The sidebar's Ctrl/Cmd-B listens on the window, theater's Escape on the
    // document.
    const onWindow = vi.fn()
    const onDocument = vi.fn()
    window.addEventListener("keydown", onWindow)
    document.addEventListener("keydown", onDocument)
    try {
      await signOutOver(<div>the app</div>)
      const field = screen.getByLabelText("Password")
      fireEvent.keyDown(field, { key: "b", ctrlKey: true })
      fireEvent.keyDown(field, { key: "Escape" })
      expect(onWindow).not.toHaveBeenCalled()
      expect(onDocument).not.toHaveBeenCalled()
      // Typing in the gate's own field still works.
      fireEvent.change(field, { target: { value: "hunter2" } })
      expect((field as HTMLInputElement).value).toBe("hunter2")
    } finally {
      window.removeEventListener("keydown", onWindow)
      document.removeEventListener("keydown", onDocument)
    }
  })

  it("lets the app's shortcuts work again once signed in", async () => {
    const onWindow = vi.fn()
    window.addEventListener("keydown", onWindow)
    try {
      const r = await signOutOver(<button>in the app</button>)
      phase = { kind: "open", status: status() }
      r.rerender(<AuthGate><button>in the app</button></AuthGate>)
      fireEvent.keyDown(screen.getByText("in the app"), { key: "b", ctrlKey: true })
      expect(onWindow).toHaveBeenCalledTimes(1)
    } finally {
      window.removeEventListener("keydown", onWindow)
    }
  })
})

describe("the hidden sidebar's divider under the login page", () => {
  // The real divider hook, which takes presses on the document in the capture
  // phase by rectangle. jsdom lays everything out at 0,0, so the login field
  // sits right inside the divider's grab band.
  const onGrab = vi.fn()
  const onDrag = vi.fn()
  const onDrop = vi.fn()
  const onReset = vi.fn()
  function Divider() {
    const ref = useDividerDrag({ onGrab, onDrag, onDrop, onReset })
    return <div ref={ref} data-testid="divider" />
  }

  it("does nothing on a press, a drag or a double-click on the login field", async () => {
    await signOutOver(<Divider />)
    const field = screen.getByLabelText("Password")
    fireEvent.pointerDown(field, { pointerId: 1, pointerType: "mouse", button: 0, clientX: 0, clientY: 0 })
    fireEvent.pointerMove(field, { pointerId: 1, pointerType: "mouse", buttons: 1, clientX: 40, clientY: 0 })
    fireEvent.pointerUp(field, { pointerId: 1, pointerType: "mouse", clientX: 40, clientY: 0 })
    fireEvent.doubleClick(field, { clientX: 0, clientY: 0 })
    expect(onGrab).not.toHaveBeenCalled()
    expect(onDrag).not.toHaveBeenCalled()
    expect(onDrop).not.toHaveBeenCalled()
    expect(onReset).not.toHaveBeenCalled()
  })

  it("still works once signed in, so the guard is the gate and not the hook", async () => {
    const r = await signOutOver(<Divider />)
    phase = { kind: "open", status: status() }
    r.rerender(<AuthGate><Divider /></AuthGate>)
    fireEvent.doubleClick(screen.getByTestId("divider"), { clientX: 0, clientY: 0 })
    expect(onReset).toHaveBeenCalledTimes(1)
  })
})

describe("the gate layer's own events", () => {
  it("keeps a file dropped on the login page from navigating away", async () => {
    await signOutOver(<div>the app</div>)
    const field = screen.getByLabelText("Password")
    const over = new Event("dragover", { bubbles: true, cancelable: true })
    field.dispatchEvent(over)
    const drop = new Event("drop", { bubbles: true, cancelable: true })
    field.dispatchEvent(drop)
    expect(over.defaultPrevented).toBe(true)
    expect(drop.defaultPrevented).toBe(true)
  })

  it("lets the gate's own controls hear presses, mouse buttons and the wheel", async () => {
    // Nothing on the way to the gate may stop these: a future gate control
    // with a hover, a tooltip or a press handler depends on them.
    await signOutOver(<div>the app</div>)
    const field = screen.getByLabelText("Password")
    const heard: string[] = []
    for (const type of ["pointerdown", "mousedown", "wheel", "compositionstart"]) {
      field.addEventListener(type, () => heard.push(type))
    }
    fireEvent.pointerDown(field)
    fireEvent.mouseDown(field)
    fireEvent.wheel(field)
    fireEvent.compositionStart(field)
    expect(heard).toEqual(["pointerdown", "mousedown", "wheel", "compositionstart"])
  })

  it("takes the hiding marks off again if something puts them back while it is up", async () => {
    await signOutOver(<div>the app</div>)
    const layer = screen.getByLabelText("Password").closest("[data-auth-gate-layer]") as HTMLElement
    layer.setAttribute("aria-hidden", "true")
    layer.setAttribute("data-base-ui-inert", "")
    layer.setAttribute("inert", "")
    await act(async () => {
      await new Promise((res) => setTimeout(res, 0))
    })
    expect(layer.hasAttribute("aria-hidden")).toBe(false)
    expect(layer.hasAttribute("data-base-ui-inert")).toBe(false)
    expect(layer.hasAttribute("inert")).toBe(false)
  })
})
