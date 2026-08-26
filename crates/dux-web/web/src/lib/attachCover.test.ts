import { describe, expect, it } from "vitest"

import { attachCover, type AttachCoverInputs } from "./attachCover"

/// The healthy steady state: owner, socket open, replay on screen, output seen.
/// Every case below is this with one or two facts moved.
const settled: AttachCoverInputs = {
  socket: "open",
  replayApplied: true,
  everReady: true,
  offline: false,
  waitExpired: false,
  isOwner: true,
  firstAttach: false,
}

describe("the settled pane", () => {
  it("is uncovered", () => {
    expect(attachCover(settled)).toEqual({ kind: "none" })
  })

  it("still shows the startup spinner when the pty has produced nothing yet", () => {
    expect(attachCover({ ...settled, everReady: false, firstAttach: true })).toEqual({
      kind: "spinner",
      wording: "starting",
    })
  })
})

describe("the cover clears on the applied replay, never on the socket opening", () => {
  it("covers a fresh mount that is open but has no screen yet", () => {
    expect(
      attachCover({ ...settled, replayApplied: false, firstAttach: true }),
    ).toEqual({ kind: "spinner", wording: "attaching" })
  })

  it("covers a reattach that is open but has no screen yet", () => {
    expect(attachCover({ ...settled, replayApplied: false })).toEqual({
      kind: "spinner",
      wording: "reconnecting",
    })
  })

  it("covers a socket that is still connecting", () => {
    expect(
      attachCover({ ...settled, socket: "connecting", replayApplied: false }),
    ).toEqual({ kind: "spinner", wording: "reconnecting" })
  })

  it("NEVER returns none while the replay is unapplied, in any combination", () => {
    for (const socket of ["connecting", "open", "closed", "failed"] as const) {
      for (const everReady of [true, false]) {
        for (const offline of [true, false]) {
          for (const waitExpired of [true, false]) {
            for (const isOwner of [true, false]) {
              for (const firstAttach of [true, false]) {
                const cover = attachCover({
                  socket,
                  replayApplied: false,
                  everReady,
                  offline,
                  waitExpired,
                  isOwner,
                  firstAttach,
                })
                expect(cover.kind).not.toBe("none")
              }
            }
          }
        }
      }
    }
  })
})

describe("the bounded wait", () => {
  it("becomes the Reconnect box once the replay wait expires", () => {
    expect(
      attachCover({ ...settled, replayApplied: false, waitExpired: true }),
    ).toEqual({ kind: "box", reason: "no-screen" })
  })

  it("does not become a box once the replay has landed", () => {
    expect(attachCover({ ...settled, waitExpired: true })).toEqual({ kind: "none" })
  })

  it("stays a spinner while globally offline, because the offline overlay owns the retry", () => {
    expect(
      attachCover({
        ...settled,
        replayApplied: false,
        waitExpired: true,
        offline: true,
      }),
    ).toEqual({ kind: "spinner", wording: "reconnecting" })
  })
})

describe("a dead socket and a watched pty", () => {
  it("a failed socket is the connection-lost box, which outranks the card", () => {
    expect(attachCover({ ...settled, socket: "failed", isOwner: false })).toEqual({
      kind: "box",
      reason: "lost",
    })
  })

  it("a failed socket while globally offline shows the cue, not the box: the overlay owns the retry", () => {
    expect(
      attachCover({ ...settled, socket: "failed", offline: true }),
    ).toEqual({ kind: "spinner", wording: "reconnecting" })
  })

  it("covers a mid-session drop even though the last open's screen is still on xterm", () => {
    // The picture is frozen rather than absent, and saying so is the whole point
    // of the reconnect cue.
    expect(attachCover({ ...settled, socket: "connecting" })).toEqual({
      kind: "spinner",
      wording: "reconnecting",
    })
  })

  it("never offers the no-screen box for a drop that HAS a screen", () => {
    expect(
      attachCover({ ...settled, socket: "connecting", waitExpired: true }),
    ).toEqual({ kind: "spinner", wording: "reconnecting" })
  })

  it("a watcher gets the take-over card, replay landed or not", () => {
    expect(attachCover({ ...settled, isOwner: false })).toEqual({ kind: "card" })
    expect(
      attachCover({ ...settled, isOwner: false, replayApplied: false }),
    ).toEqual({ kind: "card" })
  })
})
