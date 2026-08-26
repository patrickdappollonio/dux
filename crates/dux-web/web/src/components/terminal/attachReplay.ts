// THE ATTACH-AND-REPLAY MACHINE.
//
// Every (re)open of the PTY socket is answered by the server with a fresh
// repaint: the whole scrollback, as the first Binary frame. This machine owns
// what happens to that frame and to everything that races it.
//
// On a RECONNECT xterm still holds the buffer from before the drop, so writing
// the replay on top would stack a second copy of history (duplicated, garbled
// output). It resets xterm before that first reconnect frame so the replay
// rebuilds the buffer cleanly. The very FIRST open starts from an empty buffer
// (a fresh terminal), so it needs no reset; only opens after the first do.
//
// `reset()` clears every private MODE too (mouse tracking, bracketed paste,
// cursor visibility, autowrap, application cursor keys), and the child emitted
// those once at its own startup and never repeats them, so nothing on the live
// stream puts them back. The repaint therefore carries an explicit mode-restore
// tail from the server (`dux_core::pty::mode_restore_sequence`). Do NOT try to
// infer modes here from what the replay draws. Without that tail a reconnect
// landed on a full-screen agent with `mouseTrackingMode === "none"`, and the
// touch-scroll forward path (gated on exactly that) returned before it read the
// finger delta, so a finger drag did nothing at all until a hard refresh.
//
// On mobile the socket reconnects constantly, so TWO defensive guards keep a
// replay from ever stacking (Mechanism A):
//
//  1. IDEMPOTENCY BY GENERATION. The `connected` frame tags each replay with a
//     monotonic generation. This records the last generation applied and DROPS
//     any replay whose generation it has already applied: a duplicate replay,
//     or a late blob from a torn-down forwarder, becomes a no-op instead of a
//     second copy of history. An untagged replay (an older server) always
//     applies.
//
//  2. DRAIN-GATING. Before resetting and replaying it lets the PREVIOUS
//     connection's xterm write queue fully drain (the empty-write callback
//     fires only once queued writes have parsed), so a stale queued byte cannot
//     land after `reset()` and among the replay. Because that callback is
//     async, bytes arriving during the drain window are HELD and written in
//     order after the reset, so nothing is reordered or written ahead of the
//     fresh replay.
//
// It also owns the REPLAY FOCUS-REPORT WINDOW. Parsing a replay's mode-restore
// tail makes xterm volunteer a focus report of its own (measured: DECSET 1004
// makes `CoreBrowserTerminal` immediately answer through `onData`), which is
// the viewer answering for state dux-core already owns; those reports are
// dropped for the duration of the write rather than typed at the child. It is a
// COUNTER, not a flag, so overlapping replay writes cannot close the window
// early, and it is bounded by the write CALLBACK, never a timer: xterm says
// exactly when it has finished parsing the chunk. On the reconnect drain path
// only the FIRST held chunk (the replay itself) gets the window; anything after
// it is live output that raced in.
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
  /// Register the "this open's screen is now on screen" listener. Fires exactly
  /// once per open, carrying the epoch it belongs to, when the replay write's
  /// COMPLETION callback has run: xterm has parsed the bytes and the picture
  /// exists. A replay dropped by the generation dedupe counts as applied for the
  /// same reason it is dropped (the picture is already there, unchanged), or the
  /// cover would hang forever on a duplicate. Last registration wins.
  onReplayApplied: (cb: (epoch: number) => void) => void
}

export function createAttachReplay(deps: AttachReplayDeps): AttachReplay {
  const { term, replayGeneration, needsFirstFrameResize, firstFrameLanded } =
    deps

  // THE ATTACH EPOCH. A monotonically increasing pane-local integer, minted by
  // every `noteOpen()`. Everything below that is per-OPEN state is keyed to it,
  // and every write-completion callback captures the epoch it was created under
  // and returns immediately when that is no longer the live one.
  //
  // Before the epoch existed, all of this was one shared set of closure
  // variables, and a close and reopen landing mid-drain let the PREVIOUS open's
  // `term.write("", cb)` callback run against the NEW open's state: it reset the
  // terminal, flushed the old open's held chunks over the fresh replay, and
  // cleared `draining` under the new open's feet.
  let epoch = 0
  // The per-open state, valid only for `epoch`. Rebuilt by `noteOpen`, so a
  // superseded open's leftovers cannot be read by anybody: they went out of
  // scope with the epoch they belonged to.
  let awaitingRepaint = false
  let repaintNeedsReset = false
  // Set only while draining the previous connection's write queue; incoming
  // bytes are buffered here (repaint first, then any live bytes) and flushed in
  // order once the drain completes so nothing is written ahead of the
  // reset+replay.
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

  // The replay chunk specifically: the same write, wrapped in the focus-report
  // suppression window, and carrying the applied signal. The window opens before
  // the bytes go in and closes in the write's own completion callback, so it
  // covers exactly the parse of this chunk, mode-restore tail included, and not
  // a millisecond of real user focus activity either side of it.
  //
  // A zero-length frame is a real case (the server repaints even a quiet pty)
  // and needs no special handling: xterm runs the callback for an empty write,
  // measured in `lib/termwrite.xterm.test.ts`.
  const writeReplayChunk = (bytes: Uint8Array, forEpoch: number) => {
    replayWritesInFlight++
    const done = () => {
      // A superseded open's callback closes nothing: the counter it incremented
      // is the same one the LIVE open is using, so decrementing here would
      // reopen the live replay's focus-report window early. The counter is
      // repaired by the epoch swap in `noteOpen` instead.
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
    onReplayApplied(cb) {
      appliedCb = cb
    },
    noteOpen() {
      const wasFirst = firstOpen
      epoch++
      // Everything the previous open was in the middle of belongs to a byte
      // stream the server has already replaced. Its held chunks are DISCARDED
      // rather than flushed, and its in-flight write count is dropped with them:
      // the callbacks that would have decremented it are about to see a stale
      // epoch and return.
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
          // A replay already applied (duplicate, or a stale/late blob): drop it
          // entirely (no reset, no write) so it can never stack a second copy.
          // It still counts as APPLIED: the picture it would have drawn is
          // already on screen, and nothing else will ever clear this open's
          // cover.
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
            // THE CALLBACK THE EPOCH EXISTS FOR. A close and reopen landing in
            // this window leaves this closure holding the previous open's plan:
            // running it would reset the terminal the new open is painting and
            // flush a byte stream the server has already replaced.
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
