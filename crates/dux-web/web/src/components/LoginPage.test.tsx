// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { AuthStatus } from "@/lib/authApi"
import type { LoginAnswer } from "@/lib/authApi"

const signIn = vi.fn<(password: string) => Promise<LoginAnswer>>()
const retryAuthGate = vi.fn(async () => {})
vi.mock("@/lib/authGate", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/authGate")>()
  return { ...actual, signIn, retryAuthGate }
})

const { LoginPage, BlockedPage, BrokenPage } = await import("./LoginPage")

function status(overrides: Partial<AuthStatus> = {}): AuthStatus {
  return {
    password_set: true,
    required_here: true,
    signed_in: false,
    client_class: "network",
    transport_encrypted: true,
    no_auth_warning: false,
    weak_password: false,
    can_set_first_password: false,
    auth_broken: null,
    minimum_password_length: null,
    minimum_password_score: null,
    required_reason: null,
    ...overrides,
  }
}

afterEach(() => {
  cleanup()
  signIn.mockReset()
  retryAuthGate.mockClear()
  vi.useRealTimers()
})

function submit(password: string) {
  fireEvent.change(screen.getByLabelText("Password"), { target: { value: password } })
  fireEvent.click(screen.getByRole("button", { name: "Sign in" }))
}

describe("LoginPage", () => {
  it("asks for the password with the field focused", () => {
    render(<LoginPage status={status()} reason="required" />)
    const field = screen.getByLabelText("Password")
    expect(field).toHaveProperty("type", "password")
    expect(document.activeElement).toBe(field)
    expect(screen.getByRole("heading", { name: "Sign in to dux" })).toBeTruthy()
  })

  it("says in one line why this device signs in, with the setting as a chip", () => {
    render(
      <LoginPage
        status={status({
          required_reason: "`[server] tailscale` is `no`, so dux treats this device as the network.",
        })}
        reason="required"
      />,
    )
    const line = screen.getByTestId("login-required-reason")
    expect(line.textContent).toBe(
      "[server] tailscale is no, so dux treats this device as the network.",
    )
    expect(line.querySelector("code")?.textContent).toBe("[server] tailscale")
  })

  it("says nothing extra when the server gives no reason", () => {
    render(<LoginPage status={status()} reason="required" />)
    expect(screen.queryByTestId("login-required-reason")).toBeNull()
  })

  it("sends the typed password and leaves the URL alone", async () => {
    window.location.hash = "#/agent/s1"
    signIn.mockResolvedValue({ kind: "ok" })
    render(<LoginPage status={status()} reason="required" />)
    await act(async () => submit("correct horse battery staple"))
    expect(signIn).toHaveBeenCalledWith("correct horse battery staple")
    expect(window.location.hash).toBe("#/agent/s1")
  })

  it("says only that the password did not work, and clears it", async () => {
    signIn.mockResolvedValue({ kind: "wrong" })
    render(<LoginPage status={status()} reason="required" />)
    await act(async () => submit("nope"))
    expect(screen.getByRole("alert").textContent).toBe(
      "That password did not work. Try again.",
    )
    expect((screen.getByLabelText("Password") as HTMLInputElement).value).toBe("")
  })

  it("counts down a rate limit and holds the button until it ends", async () => {
    vi.useFakeTimers()
    signIn.mockResolvedValue({
      kind: "rate_limited",
      retryAfterSeconds: 3,
      from: "from the network",
    })
    render(<LoginPage status={status()} reason="required" />)
    await act(async () => submit("x"))
    // Announced once; the countdown itself is not a live region, so a screen
    // reader is not read every second.
    expect(screen.getByRole("alert").textContent).toBe("Too many sign-in attempts from the network.")
    const countdown = screen.getByText("Try again in 3 seconds.")
    expect(countdown.closest("[role=alert]")).toBeNull()
    expect(countdown.closest("[aria-live=polite],[aria-live=assertive]")).toBeNull()
    expect(screen.getByRole("button", { name: "Sign in" })).toHaveProperty("disabled", true)
    await act(async () => {
      vi.advanceTimersByTime(1000)
    })
    expect(screen.getByText("Try again in 2 seconds.")).toBeTruthy()
    await act(async () => {
      vi.advanceTimersByTime(2000)
    })
    expect(screen.getByRole("button", { name: "Sign in" })).toHaveProperty("disabled", false)
  })

  it("says to wait when the server names no wait", async () => {
    signIn.mockResolvedValue({ kind: "rate_limited", retryAfterSeconds: null, from: null })
    render(<LoginPage status={status()} reason="required" />)
    await act(async () => submit("x"))
    expect(screen.getByText("Wait a little, then try again.")).toBeTruthy()
  })

  it("says so when the server cannot be reached", async () => {
    signIn.mockResolvedValue({ kind: "unreachable" })
    render(<LoginPage status={status()} reason="required" />)
    await act(async () => submit("x"))
    expect(screen.getByRole("alert").textContent).toContain("Could not reach dux")
  })

  it("shows the server's own words for any other refusal", async () => {
    signIn.mockResolvedValue({ kind: "refused", message: "cross-origin request rejected" })
    render(<LoginPage status={status()} reason="required" />)
    await act(async () => submit("x"))
    expect(screen.getByRole("alert").textContent).toBe("cross-origin request rejected")
  })

  it("asks for the password rather than submitting an empty one", async () => {
    render(<LoginPage status={status()} reason="required" />)
    await act(async () => submit(""))
    expect(screen.getByRole("alert").textContent).toBe("Enter your password.")
    expect(signIn).not.toHaveBeenCalled()
  })

  it("warns, clearly, that a plain HTTP connection can be read", () => {
    render(<LoginPage status={status({ transport_encrypted: false })} reason="required" />)
    const warning = screen.getByRole("note", { name: "This connection is not encrypted" })
    expect(warning.textContent).toContain("read the password as you type it")
    expect(warning.textContent).toContain("session cookie")
  })

  it("shows no transport warning on an encrypted connection, or when the server did not say", () => {
    render(<LoginPage status={status({ transport_encrypted: true })} reason="required" />)
    expect(screen.queryByRole("note")).toBeNull()
    cleanup()
    render(<LoginPage status={status({ transport_encrypted: null })} reason="required" />)
    expect(screen.queryByRole("note")).toBeNull()
  })

  it.each([
    ["required", "asks for a password"],
    ["expired", "Your session ended"],
    ["signed_out", "You signed out"],
    ["password_changed", "The password changed"],
  ] as const)("explains why the page is up (%s)", (reason, text) => {
    render(<LoginPage status={status()} reason={reason} />)
    expect(screen.getByText(text, { exact: false })).toBeTruthy()
  })

  it("keeps the field and the button at the touch floor", () => {
    render(<LoginPage status={status()} reason="required" />)
    expect(screen.getByLabelText("Password").className).toContain("h-10")
    expect(screen.getByRole("button", { name: "Sign in" }).className).toContain("h-10")
  })
})

describe("BlockedPage", () => {
  it("names where the block lives and how to lift it", () => {
    render(<BlockedPage where="/home/u/.config/dux/config.toml" />)
    expect(screen.getByRole("heading", { name: "This address is blocked" })).toBeTruthy()
    const chips = [...document.querySelectorAll("[data-slot=inline-code]")].map(
      (c) => c.textContent,
    )
    expect(chips).toContain("blocked_addresses")
    expect(chips).toContain("/home/u/.config/dux/config.toml")
  })

  it("still names the setting when the server gave no location", () => {
    render(<BlockedPage where={null} />)
    expect(document.body.textContent).toContain("blocked_addresses")
    expect(document.body.textContent).toContain("config.toml")
  })

  it("asks again on Try again", async () => {
    render(<BlockedPage where={null} />)
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Try again" })))
    expect(retryAuthGate).toHaveBeenCalled()
  })
})

describe("BrokenPage", () => {
  it("names the problem the server reported", () => {
    render(<BrokenPage detail="password_hash is not a valid Argon2 PHC string" />)
    expect(screen.getByRole("heading", { name: "Sign-in is misconfigured" })).toBeTruthy()
    expect(document.body.textContent).toContain(
      "password_hash is not a valid Argon2 PHC string",
    )
  })

  it("still explains itself without a detail", () => {
    render(<BrokenPage detail="" />)
    expect(document.body.textContent).toContain("[server.auth]")
  })

  it("asks again on Try again", async () => {
    render(<BrokenPage detail="" />)
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Try again" })))
    expect(retryAuthGate).toHaveBeenCalled()
  })
})

describe("the hooks the browser journeys hold on to", () => {
  it("marks the form and the plain HTTP warning", () => {
    render(<LoginPage status={status({ transport_encrypted: false })} reason="required" />)
    const form = screen.getByTestId("login-form")
    expect(form.tagName).toBe("FORM")
    expect(form.querySelector("input[type=password]")).not.toBeNull()
    expect(form.querySelector("button[type=submit]")).not.toBeNull()
    expect(screen.getByTestId("login-insecure-warning")).toBeTruthy()
  })
})

describe("password managers", () => {
  it("get a username to file the password under", () => {
    render(<LoginPage status={status()} reason="required" />)
    const user = document.querySelector("input[autocomplete=username]") as HTMLInputElement
    expect(user).not.toBeNull()
    expect(user.value).toBe("dux")
    expect(user.form).toBe(screen.getByTestId("login-form"))
    // Out of the tab order and away from screen readers: nobody types in it.
    expect(user.tabIndex).toBe(-1)
    expect(screen.getByLabelText("Password").getAttribute("autocomplete")).toBe("current-password")
  })
})

describe("a sign-in that never answers", () => {
  it("says dux did not answer in time", async () => {
    signIn.mockResolvedValue({ kind: "unreachable", timedOut: true })
    render(<LoginPage status={status()} reason="required" />)
    await act(async () => submit("x"))
    expect(screen.getByRole("alert").textContent).toContain("did not answer in time")
  })
})

describe("after a wrong password", () => {
  it("puts the caret back in the field once it is enabled again", async () => {
    signIn.mockResolvedValue({ kind: "wrong" })
    render(<LoginPage status={status()} reason="required" />)
    const field = screen.getByLabelText("Password")
    // Somewhere else has focus when the answer arrives.
    ;(screen.getByRole("button", { name: "Sign in" }) as HTMLElement).focus()
    await act(async () => submit("nope"))
    expect(document.activeElement).toBe(field)
  })
})

describe("after the first password was set", () => {
  it("says it was set and to sign in with it", () => {
    render(<LoginPage status={status()} reason="password_set" />)
    expect(screen.getByText("Password set. Sign in with it to continue.")).toBeTruthy()
    expect(screen.queryByText(/signs every browser out/)).toBeNull()
  })
})
