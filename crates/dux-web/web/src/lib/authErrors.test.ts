import { describe, expect, it } from "vitest"

import {
  firstPasswordUnavailable,
  refusalSentence,
  storedNotInForceSentence,
  type ErrorBody,
} from "./authErrors"

function body(json: Record<string, unknown>): ErrorBody {
  return { json, text: JSON.stringify(json) }
}

describe("refusalSentence", () => {
  it("says the current password was wrong", () => {
    expect(refusalSentence(401, body({ error: "wrong_current_password" }), "change the password")).toBe(
      "The current password is not right, so nothing was changed.",
    )
  })

  it("says a weak password was refused, with zxcvbn's own feedback", () => {
    const s = refusalSentence(
      400,
      body({
        error: "weak_password",
        score: 1,
        feedback: {
          warning: "This is similar to a commonly used password.",
          suggestions: ["Add another word or two."],
        },
      }),
      "change the password",
    )
    expect(s).toBe(
      "That password is too easy to guess, so dux refused it. This is similar to a commonly used password. Add another word or two.",
    )
  })

  it("names the minimum for a short password", () => {
    expect(
      refusalSentence(400, body({ error: "password_too_short", minimum_length: 14 }), "x"),
    ).toBe("That password is too short: dux asks for at least 14 characters.")
    expect(refusalSentence(400, body({ error: "password_too_short" }), "x")).toBe(
      "That password is shorter than the minimum dux asks for.",
    )
  })

  it("explains blocked, broken and signed-out refusals in words", () => {
    expect(refusalSentence(403, body({ error: "blocked", where: "config.toml" }), "x")).toContain(
      "blocked_addresses",
    )
    expect(
      refusalSentence(503, body({ error: "auth_config_invalid", detail: "d" }), "x"),
    ).toContain("[server.auth]")
    expect(refusalSentence(401, body({ error: "auth_required" }), "x")).toBe(
      "Your session ended. Sign in again, then try once more.",
    )
  })

  it("counts down a rate limit when the wait is known", () => {
    expect(refusalSentence(429, body({ error: "rate_limited", retry_after: 7 }), "x")).toBe(
      "Too many attempts from this address. Try again in 7 seconds.",
    )
    expect(refusalSentence(429, { json: {}, text: "" }, "x")).toBe(
      "Too many attempts from this address. Wait a little, then try again.",
    )
  })

  it("never shows raw JSON for a code it does not know", () => {
    const s = refusalSentence(500, body({ error: "something_new", x: 1 }), "sign out")
    expect(s).toBe("dux refused to sign out (HTTP 500).")
    expect(s).not.toContain("{")
  })

  it("passes a plain-text refusal through, and a server-written message", () => {
    expect(
      refusalSentence(403, { json: {}, text: "cross-origin request rejected" }, "x"),
    ).toBe("cross-origin request rejected")
    expect(refusalSentence(500, body({ message: "Could not write config.toml." }), "x")).toBe(
      "Could not write config.toml.",
    )
  })

  it("never shows a JSON-looking text body even when it did not parse", () => {
    expect(refusalSentence(500, { json: {}, text: '{"error":' }, "save")).toBe(
      "dux refused to save (HTTP 500).",
    )
  })
})

describe("a password stored but not in force", () => {
  const server =
    "The new password is saved in config.toml, but dux cannot use it until these problems in that file are fixed: session_idle_seconds must be at least 1. Until then the old password stays in force."

  it("is a refusal in words: the server's sentence, which names the problems", () => {
    expect(
      refusalSentence(409, body({ error: "password_not_in_force", message: server }), "change the password"),
    ).toBe(server)
  })

  it("still says what happened when the server gave no sentence", () => {
    expect(
      refusalSentence(409, body({ error: "password_not_in_force" }), "change the password"),
    ).toBe(storedNotInForceSentence(null))
    expect(storedNotInForceSentence(null)).toContain("not in force")
  })

  it("leads with the fact for the toast, then the server's details", () => {
    const s = storedNotInForceSentence(server)
    expect(s.startsWith("The password was stored but is not in force.")).toBe(true)
    expect(s).toContain("session_idle_seconds")
  })
})

describe("firstPasswordUnavailable", () => {
  it("names where the first password can be set from", () => {
    expect(firstPasswordUnavailable(null)).toContain("`dux config set server.auth.password`")
  })

  it("adds the server's line when this device is treated as the network", () => {
    const reason = "`[server] tailscale` is `no`, so dux treats this device as the network."
    expect(firstPasswordUnavailable(reason)).toBe(`${firstPasswordUnavailable(null)} ${reason}`)
  })
})
