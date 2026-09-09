// Reconnects drain and reset xterm before applying the generation-deduped replay.
// Live bytes stay ordered, and replay-generated focus reports are suppressed.
import type { Terminal } from "@xterm/xterm"

import {
  nextAppliedGeneration,
  shouldApplyReplay,
} from "@/lib/replayGeneration"

export type AttachReplayDeps = {
  term: Terminal
  /// The generation stamped on the replay that follows the most recent
  /// `connected` frame, read at the instant the replay is applied.
  replayGeneration: () => number | null
  /// Whether the next chunk written should carry the first-frame resize
  /// callback, and the callback itself. Both belong to the resize coordinator;
  /// this machine only decides which write they ride on.
  needsFirstFrameResize: () => boolean
  firstFrameLanded: () => void
}

export type AttachReplay = {
  /// The socket's byte feed.
  onBytes: (bytes: Uint8Array) => void
  /// A (re)open landed. Mints and returns this open's ATTACH EPOCH, along with
  /// whether it was the FIRST open, which is what decides both the reset (only
  /// later opens reset) and the resize plan.
  noteOpen: () => { firstOpen: boolean; epoch: number }
  /// Whether a REPLAY chunk is being parsed right now. The `onData` gate reads
  /// this to drop a focus report the replay itself provoked.
  replayInFlight: () => boolean
  /// Retire the dedupe's high-water mark. The generation counter is
  /// process-global on the server, so a restarted run starts low and its first
  /// replay would be dropped as already applied, clearing the cover over the
  /// previous run's screen. Called when the run-identity probe can no longer
  /// vouch for the run (see `lib/serverRun.ts`), and free otherwise.
  forgetAppliedGeneration: () => void
  /// Fires once per open, carrying its epoch, when the replay write's completion
  /// callback has run and the picture exists. A replay dropped by the generation
  /// dedupe counts as applied for the same reason it is dropped, or the cover
  /// hangs forever on a duplicate. Last registration wins.
  onReplayApplied: (cb: (epoch: number) => void) => void
}

export function createAttachReplay(deps: AttachReplayDeps): AttachReplay {
  const { term, replayGeneration, needsFirstFrameResize, firstFrameLanded } =
    deps

  // The attach epoch: a pane-local integer minted by every `noteOpen()`. All
  // per-open state below is keyed to it, and every write-completion callback
  // captures the epoch it was created under and returns when that is no longer
  // live, so a close and reopen landing mid-drain cannot let the previous open's
  // callback reset the terminal the new open is painting.
  let epoch = 0
  // The per-open state, valid only for `epoch`. Rebuilt by `noteOpen`, so a
  // superseded open's leftovers cannot be read by anybody: they went out of
  // scope with the epoch they belonged to.
  let awaitingRepaint = false
  let repaintNeedsReset = false
  // Set only while draining the previous connection's write queue. Incoming bytes
  // buffer here and flush in order once it completes, so nothing is written ahead
  // of the reset and replay.
  let draining = false
  let heldChunks: Uint8Array[] = []

  let firstOpen = true
  // The dedupe mark is deliberately NOT per-epoch: it is a fact about what is on
  // the terminal, which survives every open, and that is the whole point of it.
  let lastAppliedGen: number | null = null
  // Non-zero while a REPLAY chunk is being applied to xterm.
  let replayWritesInFlight = 0
  let appliedCb: (epoch: number) => void = () => {}

  /// Report this open's replay as APPLIED, exactly once. Guarded on the epoch
  /// (a superseded open answers for nobody) and on the flag (an open reports one
  /// screen, not one per chunk that raced in behind it).
  let appliedEpoch: number | null = null
  const signalApplied = (forEpoch: number) => {
    if (forEpoch !== epoch) return
    if (appliedEpoch === forEpoch) return
    appliedEpoch = forEpoch
    appliedCb(forEpoch)
  }

  const writeChunk = (bytes: Uint8Array, forEpoch: number) => {
    if (needsFirstFrameResize()) {
      // Resize only once xterm has parsed this first frame (the repaint).
      term.write(bytes, () => {
        if (forEpoch !== epoch) return
        firstFrameLanded()
      })
    } else {
      term.write(bytes)
    }
  }

  // The replay chunk: the same write, wrapped in the focus-report suppression
  // window and carrying the applied signal. The window opens before the bytes go
  // in and closes in the write's completion callback, so it covers exactly this
  // chunk's parse, mode-restore tail included, and no real focus activity.
  //
  // A zero-length frame is a real case, since the server repaints even a quiet
  // pty, and needs nothing special: xterm runs the callback for an empty write.
  const writeReplayChunk = (bytes: Uint8Array, forEpoch: number) => {
    replayWritesInFlight++
    const done = () => {
      // A superseded open's callback closes nothing: its counter is the live
      // open's too, so decrementing reopens that replay's focus-report window
      // early. The epoch swap in `noteOpen` repairs the counter instead.
      if (forEpoch !== epoch) return
      replayWritesInFlight = Math.max(0, replayWritesInFlight - 1)
      signalApplied(forEpoch)
    }
    if (needsFirstFrameResize()) {
      term.write(bytes, () => {
        const live = forEpoch === epoch
        done()
        if (live) firstFrameLanded()
      })
    } else {
      term.write(bytes, done)
    }
  }

  return {
    replayInFlight: () => replayWritesInFlight > 0,
    forgetAppliedGeneration() {
      lastAppliedGen = null
    },
    onReplayApplied(cb) {
      appliedCb = cb
    },
    noteOpen() {
      const wasFirst = firstOpen
      epoch++
      // Everything the previous open was mid-way through belongs to a byte
      // stream the server has replaced, so its held chunks are discarded rather
      // than flushed and its in-flight write count goes with them: the callbacks
      // that would decrement it are about to see a stale epoch and return.
      awaitingRepaint = true
      draining = false
      heldChunks = []
      replayWritesInFlight = 0
      appliedEpoch = null
      // Only opens AFTER the first reset the buffer, since the first open starts
      // from an empty terminal.
      if (firstOpen) {
        firstOpen = false
        repaintNeedsReset = false
      } else {
        repaintNeedsReset = true
      }
      return { firstOpen: wasFirst, epoch }
    },
    onBytes(bytes) {
      const forEpoch = epoch
      // Mid-drain: hold everything (the repaint plus any live bytes that raced
      // in) so it lands in order after reset(), never ahead of the fresh
      // replay.
      if (draining) {
        heldChunks.push(bytes)
        return
      }
      if (awaitingRepaint) {
        awaitingRepaint = false
        const gen = replayGeneration()
        if (!shouldApplyReplay(gen, lastAppliedGen)) {
          // A replay already applied is dropped whole, with no reset and no
          // write, so it can never stack a second copy. It still counts as
          // applied: its picture is on screen, and nothing else clears the cover.
          signalApplied(forEpoch)
          return
        }
        lastAppliedGen = nextAppliedGeneration(gen, lastAppliedGen)
        if (repaintNeedsReset) {
          // Reconnect replay: drain the previous connection's queue, then reset
          // and replay (plus any raced-in live bytes) in order.
          draining = true
          heldChunks = [bytes]
          term.write("", () => {
            // The callback the epoch exists for: a close and reopen inside this
            // window leaves the closure holding the previous open's plan, which
            // would reset the terminal the new open is painting.
            if (forEpoch !== epoch) return
            term.reset()
            const chunks = heldChunks
            heldChunks = []
            draining = false
            // The FIRST held chunk is the replay itself (it seeded the array
            // above); anything after it is live output that raced in, so only
            // the first gets the focus-report suppression window.
            chunks.forEach((c, i) => {
              if (i === 0) writeReplayChunk(c, forEpoch)
              else writeChunk(c, forEpoch)
            })
          })
        } else {
          // Very first open: the buffer is already empty, so no reset or drain
          // is needed. Write the repaint straight through.
          writeReplayChunk(bytes, forEpoch)
        }
        return
      }
      writeChunk(bytes, forEpoch)
    },
  }
}
