// @vitest-environment jsdom
import { describe, expect, it } from "vitest"

import type { Terminal } from "@xterm/xterm"

import { createAttachReplay } from "./attachReplay"

// A terminal faithful to the three things this machine depends on: writes are
// QUEUED (the callback fires when the queue is pumped, which is what makes the
// drain gate a real gate rather than a formality), `reset()` is observable, and
// the write order is recorded.
class TermFake {
  log: string[] = []
  private queue: (() => void)[] = []
  write(data: unknown, cb?: () => void) {
    const text =
      typeof data === "string" ? data : new TextDecoder().decode(data as never)
    this.log.push(text === "" ? "drain" : `write:${text}`)
    if (cb) this.queue.push(cb)
  }
  reset() {
    this.log.push("reset")
  }
  /// Deliver every queued write callback, the way xterm does once it has parsed
  /// what it was given. Re-entrant writes queue for the NEXT pump.
  pump() {
    const due = this.queue
    this.queue = []
    for (const cb of due) cb()
  }
}

function bytes(s: string): Uint8Array {
  return new TextEncoder().encode(s)
}

function setup(gen: () => number | null = () => null) {
  const term = new TermFake()
  let firstFrames = 0
  let needsFirstFrame = true
  const attach = createAttachReplay({
    term: term as unknown as Terminal,
    replayGeneration: gen,
    needsFirstFrameResize: () => needsFirstFrame,
    firstFrameLanded: () => {
      firstFrames++
      needsFirstFrame = false
    },
  })
  return {
    term,
    attach,
    firstFrames: () => firstFrames,
    settleFirstFrame: () => {
      needsFirstFrame = false
    },
  }
}

describe("the very first open", () => {
  it("writes the replay straight through, with no reset and no drain", () => {
    const { term, attach } = setup()
    expect(attach.noteOpen()).toMatchObject({ firstOpen: true })
    attach.onBytes(bytes("hello"))
    expect(term.log).toEqual(["write:hello"])
  })

  it("hangs the first-frame resize off the replay write's own callback", () => {
    const { term, attach, firstFrames } = setup()
    attach.noteOpen()
    attach.onBytes(bytes("hello"))
    expect(firstFrames()).toBe(0)
    term.pump()
    expect(firstFrames()).toBe(1)
  })

  it("holds the focus-report window open for exactly the replay's parse", () => {
    const { term, attach } = setup()
    attach.noteOpen()
    expect(attach.replayInFlight()).toBe(false)
    attach.onBytes(bytes("hello"))
    expect(attach.replayInFlight()).toBe(true)
    term.pump()
    expect(attach.replayInFlight()).toBe(false)
  })

  it("opens no window for ordinary live output after the replay", () => {
    const { term, attach } = setup()
    attach.noteOpen()
    attach.onBytes(bytes("replay"))
    term.pump()
    attach.onBytes(bytes("live"))
    expect(attach.replayInFlight()).toBe(false)
    expect(term.log).toEqual(["write:replay", "write:live"])
  })
})

describe("a reconnect", () => {
  it("drains, resets, and only then writes the replay", () => {
    const { term, attach } = setup()
    attach.noteOpen()
    attach.onBytes(bytes("first"))
    term.pump()
    expect(attach.noteOpen()).toMatchObject({ firstOpen: false })
    attach.onBytes(bytes("replay"))
    // Nothing has been written yet: the previous connection's queue is
    // draining.
    expect(term.log).toEqual(["write:first", "drain"])
    term.pump()
    expect(term.log).toEqual(["write:first", "drain", "reset", "write:replay"])
  })

  it("HOLDS bytes that race in mid-drain and writes them in order after the reset", () => {
    const { term, attach } = setup()
    attach.noteOpen()
    attach.onBytes(bytes("first"))
    term.pump()
    attach.noteOpen()
    attach.onBytes(bytes("replay"))
    attach.onBytes(bytes("raced-1"))
    attach.onBytes(bytes("raced-2"))
    term.pump()
    expect(term.log).toEqual([
      "write:first",
      "drain",
      "reset",
      "write:replay",
      "write:raced-1",
      "write:raced-2",
    ])
  })

  it("gives the focus-report window to the replay chunk ONLY, not to the raced-in bytes", () => {
    const { term, attach, settleFirstFrame } = setup()
    settleFirstFrame()
    attach.noteOpen()
    attach.onBytes(bytes("first"))
    term.pump()
    attach.noteOpen()
    attach.onBytes(bytes("replay"))
    attach.onBytes(bytes("raced"))
    term.pump()
    // Both chunks are now queued; only the replay's write carries the counter,
    // so exactly one close is pending.
    expect(attach.replayInFlight()).toBe(true)
    term.pump()
    expect(attach.replayInFlight()).toBe(false)
  })
})

describe("replay idempotency by generation", () => {
  it("drops a replay whose generation was already applied, whole: no reset, no write", () => {
    let gen = 1
    const { term, attach } = setup(() => gen)
    attach.noteOpen()
    attach.onBytes(bytes("replay-1"))
    term.pump()
    // The same generation again (a duplicate replay, or a late blob from a
    // torn-down forwarder).
    attach.noteOpen()
    attach.onBytes(bytes("replay-1-again"))
    term.pump()
    expect(term.log).toEqual(["write:replay-1"])
    // And the connection still works: the next real generation applies.
    gen = 2
    attach.noteOpen()
    attach.onBytes(bytes("replay-2"))
    term.pump()
    term.pump()
    expect(term.log).toContain("write:replay-2")
  })

  it("always applies an UNTAGGED replay, because an older server sends no generation", () => {
    const { term, attach } = setup(() => null)
    attach.noteOpen()
    attach.onBytes(bytes("a"))
    term.pump()
    attach.noteOpen()
    attach.onBytes(bytes("b"))
    term.pump()
    term.pump()
    expect(term.log.filter((l) => l.startsWith("write:"))).toEqual([
      "write:a",
      "write:b",
    ])
  })

  it("leaves the machine usable after a dropped replay: live bytes still land", () => {
    const { term, attach } = setup(() => 7)
    attach.noteOpen()
    attach.onBytes(bytes("replay"))
    term.pump()
    attach.noteOpen()
    attach.onBytes(bytes("dropped"))
    attach.onBytes(bytes("live"))
    expect(term.log).toEqual(["write:replay", "write:live"])
  })
})

// ATTACH EPOCHS. Every open mints one, and every piece of per-open state is
// keyed to it. Before this, `awaitingRepaint`, `draining`, `heldChunks`,
// `repaintNeedsReset` and `lastAppliedGen` were ONE set of closure variables
// shared across every open of the pane's lifetime, so a close and reopen landing
// mid-drain let the PREVIOUS open's write callback run against the NEW open's
// state: it reset the terminal, flushed the old open's held chunks, and cleared
// `draining` under the new open's feet.
describe("attach epochs", () => {
  it("mints a new epoch per open and reports it on the applied signal", () => {
    const { term, attach } = setup()
    const applied: number[] = []
    attach.onReplayApplied((epoch) => applied.push(epoch))
    const first = attach.noteOpen()
    attach.onBytes(bytes("one"))
    term.pump()
    expect(applied).toEqual([first.epoch])
    const second = attach.noteOpen()
    expect(second.epoch).toBeGreaterThan(first.epoch)
  })

  it("counts a generation-deduped replay as applied, or the cover would hang on a duplicate", () => {
    const { term, attach } = setup(() => 4)
    const applied: number[] = []
    attach.onReplayApplied((epoch) => applied.push(epoch))
    attach.noteOpen()
    attach.onBytes(bytes("replay"))
    term.pump()
    expect(applied).toHaveLength(1)
    // The duplicate is dropped whole (no reset, no write) and must STILL report
    // applied, for its own epoch: nothing else will ever clear this open's cover.
    const dup = attach.noteOpen()
    attach.onBytes(bytes("replay-again"))
    expect(applied).toEqual([applied[0], dup.epoch])
  })

  it("fires the applied signal exactly once per open", () => {
    const { term, attach } = setup()
    const applied: number[] = []
    attach.onReplayApplied((epoch) => applied.push(epoch))
    attach.noteOpen()
    attach.onBytes(bytes("replay"))
    attach.onBytes(bytes("live"))
    term.pump()
    term.pump()
    expect(applied).toHaveLength(1)
  })

  // (a) The reopen lands between noteOpen() and the first binary frame.
  it("a reopen before the first frame answers for the NEW epoch alone", () => {
    const { term, attach } = setup()
    const applied: number[] = []
    attach.onReplayApplied((epoch) => applied.push(epoch))
    attach.noteOpen()
    attach.onBytes(bytes("first"))
    term.pump()
    applied.length = 0
    // Superseded before a byte arrived.
    attach.noteOpen()
    const live = attach.noteOpen()
    attach.onBytes(bytes("second"))
    term.pump()
    term.pump()
    expect(applied).toEqual([live.epoch])
    expect(term.log.at(-1)).toBe("write:second")
  })

  // (b) The reopen lands MID-DRAIN, and the old drain callback fires afterwards.
  it("a drain callback from a superseded open is inert, and discards its held chunks", () => {
    const { term, attach } = setup()
    const applied: number[] = []
    attach.onReplayApplied((epoch) => applied.push(epoch))
    attach.noteOpen()
    attach.onBytes(bytes("first"))
    term.pump()
    applied.length = 0
    term.log.length = 0

    // A reconnect: this open drains before resetting.
    attach.noteOpen()
    attach.onBytes(bytes("stale-replay"))
    expect(term.log).toEqual(["drain"])
    // The socket drops and reopens BEFORE that drain callback runs.
    const fresh = attach.noteOpen()
    attach.onBytes(bytes("fresh-replay"))
    // Now both drains complete, oldest first.
    term.pump()
    term.pump()
    term.pump()

    // The superseded open's drain wrote nothing: its held chunk belongs to a
    // byte stream the server has already replaced.
    expect(term.log).not.toContain("write:stale-replay")
    expect(term.log).toContain("write:fresh-replay")
    // And exactly one applied signal, for the live epoch.
    expect(applied).toEqual([fresh.epoch])
  })

  // (c) The reopen lands mid-`writeReplayChunk`, and the old completion callback
  // fires afterwards.
  it("a replay write callback from a superseded open neither re-signals nor reopens the focus window", () => {
    const { term, attach } = setup()
    const applied: number[] = []
    attach.onReplayApplied((epoch) => applied.push(epoch))
    attach.noteOpen()
    attach.onBytes(bytes("first"))
    // The first open's replay is queued but NOT parsed yet.
    applied.length = 0
    const fresh = attach.noteOpen()
    attach.onBytes(bytes("second"))
    // Pump everything: the superseded write's callback runs first.
    term.pump()
    term.pump()
    term.pump()
    expect(applied).toEqual([fresh.epoch])
    // The window is closed: a superseded callback must not leave the counter
    // stuck open, and must not have decremented the live epoch's own count.
    expect(attach.replayInFlight()).toBe(false)
  })

  it("keeps the generation dedupe on the MACHINE rather than per epoch, so a live open still applies", () => {
    let gen = 1
    const { term, attach } = setup(() => gen)
    attach.noteOpen()
    attach.onBytes(bytes("gen-1"))
    term.pump()
    // A new open whose replay carries a NEW generation still applies after a
    // superseded sibling: the dedupe mark is the machine's, not the epoch's, and
    // a stale open must not have banked a generation the live one then drops.
    attach.noteOpen()
    gen = 2
    attach.noteOpen()
    attach.onBytes(bytes("gen-2"))
    term.pump()
    term.pump()
    expect(term.log).toContain("write:gen-2")
  })
})
