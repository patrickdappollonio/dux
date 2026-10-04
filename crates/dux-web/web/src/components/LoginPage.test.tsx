// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { AuthStatus } from "@/lib/authApi"
import type { LoginAnswer } from "@/lib/authApi"

const signIn = vi.fn<(password: string) => Promise<LoginAnswer>>()
const probeAuth = vi.fn(async () => {})
vi.mock("@/lib/authGate", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/authGate")>()
  return { ...actual, signIn, probeAuth }
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
    ...overrides,
  }
}

afterEach(() => {
  cleanup()
  signIn.mockReset()
  probeAuth.mockClear()
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
    signIn.mockResolvedValue({ kind: "rate_limited", retryAfterSeconds: 3 })
    render(<LoginPage status={status()} reason="required" />)
    await act(async () => submit("x"))
    expect(screen.getByRole("alert").textContent).toContain("Try again in 3 seconds.")
    expect(screen.getByRole("button", { name: "Sign in" })).toHaveProperty("disabled", true)
    await act(async () => {
      vi.advanceTimersByTime(1000)
    })
    expect(screen.getByRole("alert").textContent).toContain("Try again in 2 seconds.")
    await act(async () => {
      vi.advanceTimersByTime(2000)
    })
    expect(screen.getByRole("button", { name: "Sign in" })).toHaveProperty("disabled", false)
  })

  it("says to wait when the server names no wait", async () => {
    signIn.mockResolvedValue({ kind: "rate_limited", retryAfterSeconds: null })
    render(<LoginPage status={status()} reason="required" />)
    await act(async () => submit("x"))
    expect(screen.getByRole("alert").textContent).toContain("Wait a little")
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

  it("does not submit an empty password", async () => {
    render(<LoginPage status={status()} reason="required" />)
    await act(async () => submit(""))
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
    expect(probeAuth).toHaveBeenCalled()
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
    expect(probeAuth).toHaveBeenCalled()
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
