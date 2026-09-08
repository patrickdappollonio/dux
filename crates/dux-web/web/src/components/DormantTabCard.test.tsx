// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

// The card only calls `startDormantTab` from the store; stub it so we can render
// in isolation and assert the docs link.
vi.mock("@/lib/store", () => ({ startDormantTab: vi.fn() }))

const { DormantTabCard } = await import("./DormantTabCard")

afterEach(cleanup)

describe("DormantTabCard", () => {
  // Launching a dormant tab picks up that provider's most recent conversation
  // when it is the sole live-or-launching tab of that provider, so the card must
  // state that rule rather than claim a conversation is never restored.
  it("states the per-provider resume rule", () => {
    render(<DormantTabCard sessionId="s1" tabId="b2" provider="claude" />)
    const body = document.body.textContent ?? ""
    expect(body).toMatch(/most recent conversation/)
    expect(body).toMatch(/already running/)
    expect(body).not.toMatch(/doesn’t restore/)
  })

  // A tab that is simply not running does not reach this card at all any more
  // (`DormantTabSurface` sends it to the idle screen), but the card must still
  // be honest if it is rendered without one: no failure sentence out of nowhere.
  it("says nothing about a failure for a tab that simply is not running", () => {
    render(<DormantTabCard sessionId="s1" tabId="b2" provider="claude" />)
    expect(document.body.textContent ?? "").not.toMatch(/last run/i)
  })

  // Neutral on purpose: a non-zero exit is often the user quitting the CLI, so
  // the sentence reports what dux saw and what it therefore did not do, and
  // never accuses the provider of crashing.
  it("explains, without accusing, why a failed tab waits for a press", () => {
    render(
      <DormantTabCard sessionId="s1" tabId="b2" provider="claude" lastRunFailed />,
    )
    const body = document.body.textContent ?? ""
    // The flag with no verdict beside it is an OLDER SERVER: this build always
    // publishes the two together. The generic sentence stays for exactly that
    // case (and for a verdict this build cannot word), and is unreachable
    // otherwise.
    expect(body).toMatch(/last run ended with an error or a non-zero exit/i)
    expect(body).toMatch(/didn\u2019t start it again on its own/i)
    expect(body).not.toMatch(/crash/i)
    expect(body).not.toMatch(/Last output/)
    // The way forward is unchanged, so the rest of the card still reads the same.
    expect(body).toMatch(/most recent conversation/)
    expect(screen.getByText("Start session")).toBeTruthy()
  })

  // The case this whole feature exists for: `codex resume --last` exits 1 in a
  // fifth of a second and says why on its way out. The card must carry both the
  // ending and that line, or the user has nothing to act on.
  it("states the ending, its age, and the run's last output", () => {
    render(
      <DormantTabCard
        sessionId="s1"
        tabId="b2"
        provider="codex"
        lastRunFailed
        lastRunVerdict={{
          ending: "exited",
          status: 1,
          ended_seconds_ago: 120,
          excerpt: [
            "Error: Failed to resume session",
            "already has an active writer",
          ],
        }}
      />,
    )
    const body = document.body.textContent ?? ""
    expect(body).toMatch(/last run exited with status 1 about 2 minutes ago/i)
    expect(body).toMatch(/didn\u2019t start it again on its own/i)
    expect(body).toMatch(/Last output/)
    expect(body).toMatch(/already has an active writer/)
    // Unchanged below the diagnosis.
    expect(body).toMatch(/most recent conversation/)
    expect(screen.getByText("Start session")).toBeTruthy()
  })

  // A launch that never came up has no screen to excerpt, so the spawn error is
  // the whole diagnosis and no empty output block is drawn.
  it("states a failed launch's error and shows no output block", () => {
    render(
      <DormantTabCard
        sessionId="s1"
        tabId="b2"
        provider="claude"
        lastRunFailed
        lastRunVerdict={{
          ending: "launch_failed",
          error: "no such file or directory",
          ended_seconds_ago: 3,
          excerpt: [],
        }}
      />,
    )
    const body = document.body.textContent ?? ""
    expect(body).toMatch(
      /could not be launched moments ago: no such file or directory/i,
    )
    expect(body).not.toMatch(/Last output/)
  })

  // A clean exit that was over in a blink counts as a bad ending, and the card
  // has to say why a status of 0 is being treated as a failure at all.
  it("explains a clean exit that was over in a blink", () => {
    render(
      <DormantTabCard
        sessionId="s1"
        tabId="b2"
        provider="claude"
        lastRunFailed
        lastRunVerdict={{
          ending: "rapid_clean_exit",
          ended_seconds_ago: 0,
          excerpt: [],
        }}
      />,
    )
    expect(document.body.textContent ?? "").toMatch(
      /ended moments ago in under five seconds with status 0, which dux treats as a run that never came up/i,
    )
  })

  // An `exited` verdict with no status is a server that did not report one.
  // Printing "status 0" there would invent the single most misleading number
  // available: zero is what a run that SUCCEEDED exits with.
  it("never fabricates a status for an exited verdict that carries none", () => {
    render(
      <DormantTabCard
        sessionId="s1"
        tabId="b2"
        provider="claude"
        lastRunFailed
        lastRunVerdict={{
          ending: "exited",
          status: null,
          ended_seconds_ago: 5,
          excerpt: [],
        }}
      />,
    )
    const body = document.body.textContent ?? ""
    expect(body).not.toMatch(/status 0/)
    expect(body).toMatch(/ended with an error or a non-zero exit/i)
  })

  // A kind this build has never heard of must not reach a person as a wire
  // string; it falls back to the sentence the card had before verdicts existed.
  it("falls back to the generic sentence for an unknown ending", () => {
    render(
      <DormantTabCard
        sessionId="s1"
        tabId="b2"
        provider="claude"
        lastRunFailed
        lastRunVerdict={{
          ending: "signalled_by_the_kernel",
          ended_seconds_ago: 5,
          excerpt: [],
        }}
      />,
    )
    const body = document.body.textContent ?? ""
    expect(body).toMatch(/ended with an error or a non-zero exit/i)
    expect(body).not.toMatch(/signalled_by_the_kernel/)
  })

  it("links to the resume docs, safely, in a new tab", () => {
    render(<DormantTabCard sessionId="s1" tabId="b2" provider="claude" />)
    const link = screen.getByRole("link", { name: /how resume works/i })
    expect(link.getAttribute("href")).toBe(
      "https://getdux.app/docs/agent-tabs#how-resume-works",
    )
    expect(link.getAttribute("target")).toBe("_blank")
    expect(link.getAttribute("rel")).toContain("noopener")
    expect(link.getAttribute("rel")).toContain("noreferrer")
  })
})
