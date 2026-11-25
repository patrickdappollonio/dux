import { describe, expect, it } from "vitest"

import {
  attachCover,
  coverOwnsThePane,
  type AttachCoverInputs,
} from "./attachCover"

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
  handoffGrace: false,
}

describe("a socket that failed while its tab may be handing over", () => {
  const failed: AttachCoverInputs = { ...settled, socket: "failed" }

  it("holds the picture it has rather than painting the box", () => {
    // A provider that exits cleanly takes its socket down at once and its row a
    // moment later, so this is what the pane shows for that moment.
    expect(attachCover({ ...failed, handoffGrace: true })).toEqual({ kind: "none" })
  })

  it("paints the box once the hold is over and the pane is still here", () => {
    expect(attachCover(failed)).toEqual({ kind: "box", reason: "lost" })
  })

  it("holds nothing over a watcher's card while the app is offline", () => {
    // The offline overlay owns that signal, and the hold has nothing to do with
    // it: a socket that failed while globally offline was never handing over.
    expect(attachCover({ ...failed, offline: true, isOwner: false })).toEqual({
      kind: "card",
    })
  })
})

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
                // AND THE BOX ONLY EVER CLAIMS WHAT IT CAN SEE. Its wording is
                // about a healthy socket that never sent a screen, so a socket
                // that is not open must never produce it: the visible clock is
                // reset by `pty.onOpen` alone and keeps running across a drop,
                // so a slow reconnect must not slam an opaque panel over a
                // perfectly good frozen picture.
                if (cover.kind === "box" && cover.reason === "no-screen") {
                  expect(socket).toBe("open")
                }
              }
            }
          }
        }
      }
    }
  })
})

describe("the bounded wait", () => {
  it("stays a spinner past the wait while the socket is DOWN", () => {
    for (const socket of ["connecting", "closed"] as const) {
      expect(
        attachCover({ ...settled, socket, replayApplied: false, waitExpired: true }),
      ).toEqual({ kind: "spinner", wording: "reconnecting" })
    }
  })

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

describe("coverOwnsThePane", () => {
  it("gives the whole pane to the card and the Reconnect box", () => {
    expect(coverOwnsThePane({ kind: "card" })).toBe(true)
    expect(coverOwnsThePane({ kind: "box", reason: "lost" })).toBe(true)
    expect(coverOwnsThePane({ kind: "box", reason: "no-screen" })).toBe(true)
  })

  it("leaves the pane to the terminal under a spinner and under nothing", () => {
    // The spinner flashes on every short reconnect, so counting it would flick
    // the phone's chrome in and out on a blip.
    expect(coverOwnsThePane({ kind: "spinner", wording: "attaching" })).toBe(
      false,
    )
    expect(coverOwnsThePane({ kind: "none" })).toBe(false)
  })
})
