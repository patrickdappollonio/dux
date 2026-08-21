# TerminalPane rewrite contract (TEMPORARY SCAFFOLDING)

**What this is.** Scaffolding for the upcoming one-arc rewrite of `TerminalPane`'s
wiring, per the approved plan. It inventories every behavior the pane must honor:
each measured fact, each trap actually hit, each deliberate divergence, and the fix
chosen, with the test that pins it. Reviewers of the rewrite walk this document
entry by entry and check the new wiring against each item.

**What this is not.** It is not documentation and it is not permanent. It is
DELETED after the rewrite's final adversarial review passes and Patrick approves.
The durable home for every entry here is the code comment at the cited line, the
pinning test, and the CLAUDE.md web-terminal tenets. Do not "preserve" this file,
do not link to it, and do not let it drift into a second source of truth.

**Conventions.** `TP` is `src/components/TerminalPane.tsx` (paths are relative to
`crates/dux-web/web/` unless they start with `crates/`). Line numbers were
against commit `39f5c2ce` and have been RE-CITED against the rebuilt wiring: the
pane's internals now live in `src/components/terminal/` (the lifecycle hook, the
live-settings container, the six named machines, and the two pane-adjacent
units), and every citation below names the module the behavior moved into. The
entries' SUBSTANCE is unchanged except where the rebuild's report lists a
deviation. "Pinned:" names the test file and the test (or describe block)
that fails if the behavior regresses; "unpinned" means a comment is currently the
only guard and the rewrite's review must weigh whether that is still acceptable.

---

## A. Sizing and layout

1. **Padding lives on the host div, one layer outside the element xterm opens into.**
   Trap: FitAddon measures the open target's parent via `getComputedStyle().height`,
   which under Tailwind's global `box-sizing: border-box` includes padding; padding on
   the measured element inflated availableHeight by 16px and minted a phantom row
   (~16 of every 17 window heights) that rendered clipped under the status bar, and the
   PTY was told about it, so bottom-anchored TUIs drew into an invisible row.
   Fix: TP:100-111 (`hostRef` padded, `containerRef` unpadded), TP:762-770.
   Pinned: unpinned (comment only).

2. **The pane is its own clip boundary (`overflow-hidden`).**
   Trap: between a container resize and the next-rAF refit xterm holds its previous,
   possibly larger size; a one-frame overflow escaping to a scrollable ancestor flashes
   scrollbars and oscillates the layout (scrollbar shrinks box, RO fires, refit, repeat).
   Fix: TP:689-706. Pinned: unpinned.

3. **The PTY resize is debounced to one send at 200ms, and the local refit is HELD for
   that window and released with it.**
   Trap: each PTY resize is a SIGWINCH (full child repaint); per-frame sends during a
   divider drag are the resize jitter, which is why the send is debounced. But the refit
   used to run per animation frame anyway, so for the whole drag the local grid ran ahead
   of the child's, and the child's repaints landed in a geometry the viewer no longer had
   (measured on a simulated drag: 13 transcript rows duplicated permanently into local
   scrollback; zero with the fit held). The debounce is therefore the SECOND hold source
   alongside the touch gesture, coalesced, last geometry wins. Accepted tradeoff, the
   mirror of A11's: the canvas letterboxes for up to `RESIZE_SEND_DEBOUNCE_MS` during a
   drag. Fix: src/components/terminal/constants.ts:26-32 (`RESIZE_SEND_DEBOUNCE_MS`),
   src/components/terminal/resizeCoordinator.ts:150-155 (`fitHeldByDebounce`,
   `debouncePending`), src/components/terminal/resizeCoordinator.ts:195-228
   (`sendSize` releases the pair, `armDebounce`),
   src/components/terminal/resizeCoordinator.ts:230-242 (`fitOrHold` holds on either
   source), src/components/terminal/resizeCoordinator.ts:364-371 (RO routes through
   `fitOrHold`).
   Pinned: `components/TerminalPane.test.tsx` "a resize outside any gesture still sends
   through the normal debounce"; `src/components/terminal/resizeCoordinator.test.ts`
   "the resize coordinator's debounce hold" (all three: no refit during an observer
   burst then one fit+send at the settle, a gesture starting inside the window inherits
   the parked fit, a direct send does not cause a double fit).

4. **Geometry reaches the PTY from exactly one place: xterm's own `onResize` event.**
   Trap: the font-load refit re-grids with no container resize anywhere; nothing was
   watching, the PTY kept the fallback-metrics size, and on a phone a copy of the agent's
   cursor-relative status line was left behind on every redraw (the size healed at the
   next container resize; the scrollback garbage did not). Fix: src/components/terminal/resizeCoordinator.ts:330-347.
   Pinned: `components/TerminalPane.test.tsx` "sends the resize when a FONT LOAD re-grids
   the terminal, with no container resize".

5. **The last-told-size record books only what actually went on the wire.**
   Trap: two things swallow a resize silently (the owner gate, and the socket when not
   OPEN, which is every reconnect); a swallowed send booked as sent makes the dedupe
   suppress the re-assert forever. The take-over arc leans on the SAME boolean for a
   second job: the armed take-over intent is consumed only when `sendResize` confirms the
   frame went out, so a claim the socket discarded stays armed instead of being spent on
   nothing. Fix: src/components/terminal/resizeCoordinator.ts (`sendOwned`),
   `src/lib/ptySocket.ts` (`sendResize` returns whether the frame went out, and takes the
   optional `takeover` flag), src/components/terminal/useTerminalLifecycle.ts (the
   wrapper that reads the answer).
   Pinned: `components/TerminalPane.test.tsx` "does not record a resize the owner gate
   dropped, so it is re-sent once ownership returns" and "does not record a resize the
   SOCKET dropped, so it is re-sent once it reopens"; `src/lib/ptySocket.test.ts`
   "reports a resize it could NOT write, rather than dropping it silently".

6. **The initial PTY resize waits for the first PTY frame (the repaint) to fully parse.**
   Trap: resizing before or mid-repaint races a half-painted buffer and leaves the
   cursor and bottom-anchored prompt in the wrong rows. Fix: src/components/terminal/resizeCoordinator.ts:267-288
   (`firstFrameLanded` fired from the write callback), src/components/terminal/resizeCoordinator.ts:354-360 (250ms fallback for
   a session that emits no first frame). Pinned: `components/TerminalPane.test.tsx`
   "reports the re-grid caused by the first-frame handler's OWN fit".

7. **The very first open jiggles the width (down one column and back, 60ms apart).**
   Trap: a same-size resize is a kernel no-op (no SIGWINCH), so when the PTY already
   matches the viewport the agent never repaints over the imperfect initial snapshot.
   Fix: src/components/terminal/resizeCoordinator.ts:294-311, `src/lib/firstFrameResize.ts:24-26`.
   Pinned: `src/lib/firstFrameResize.test.ts` "jiggles on the very first open to repaint
   over the initial snapshot"; `components/TerminalPane.test.tsx` "still jiggles on the
   very first open (the deliberate first-frame bypass)" and "carries a re-grid landing
   INSIDE the jiggle window through to the PTY".

8. **A reconnect's first frame sends a single plain resize and never jiggles**, and
   since the take-over arc, that single frame is ALSO the carrier of a take-over claim
   when one is armed (the flag rides it; the plan itself is unchanged). Original entry:
   Trap: mobile reconnects constantly; a jiggle forces two full-screen repaints at two
   widths on every one. The single resize still re-asserts ownership and is a kernel
   no-op when the size is unchanged. Fix: src/components/terminal/resizeCoordinator.ts:312-322, `src/lib/firstFrameResize.ts`.
   Pinned: `src/lib/firstFrameResize.test.ts` "sends a single resize on a reconnect...";
   `components/TerminalPane.test.tsx` "still sends one plain resize on a RECONNECT's
   first frame, and does not jiggle".

9. **`lastRows`/`lastCols` are seeded right after the mount-time fit.**
   Trap: ResizeObserver fires an initial callback on observe; unseeded, it would send a
   racing resize before the first paint. Fix: src/components/terminal/resizeCoordinator.ts:348-354.
   Pinned: NEW at the rebuild, `src/components/terminal/resizeCoordinator.test.ts`
   "sends nothing when the geometry has not moved since the last send" (and the
   first-frame suite exercises it too).

10. **A foreground return force-resends the size, bypassing the dedupe, drain-gated.**
    Trap: the PTY is shared; another client resized it while this tab was hidden or
    unfocused, so the cached last-size wrongly suppresses the re-assert; and a return can
    coincide with the replay still streaming, where a mid-replay resize corrupts scroll
    position. Since the take-over arc it is also the standing recovery for a resize the
    SERVER refused (the record books "written to the socket" and nothing more), and it is
    gated on ownership like every other send, so a watcher's alt-tab volunteers nothing.
    Fix: src/components/terminal/useTerminalLifecycle.ts (`resyncToForeground`: visibilitychange AND window focus,
    150ms debounce, `term.write("")` drain gate, forced send). Pinned:
    `components/TerminalPane.test.tsx` "still re-asserts an UNCHANGED size on a
    foreground return (the dedupe bypass)".

11. **NEW at 39f5c2ce: the local refit and the PTY resize are one atomic pair held by a
    touch gesture, flushed together at the lift, refit first, exactly one fit.**
    Trap (measured, xterm 6.0.0): `Buffer.resize` sets `scrollBottom = newRows - 1`
    unconditionally and `scrollTop = 0`, over the normal AND alt buffer, so every
    `fit.fit()` that changes the grid silently resets DECSTBM on both buffers. A refit
    mid-gesture (the scroll-start blur collapses the keyboard, the viewport grows under
    the finger, the RO refits every frame) hands a region-relative mouse-tracking pager a
    viewer whose margins are gone; its repaint stamps one line per forwarded wheel notch,
    the repeated-line bug on phones. Holding only the SIGWINCH was not enough.
    Fix: src/components/terminal/resizeCoordinator.ts:146-149 (`fitHeldByGesture`), src/components/terminal/resizeCoordinator.ts:230-266
    (`fitOrHold`, `fitAndSend`), src/components/terminal/resizeCoordinator.ts:405-428
    (`flushHeld`, the lift's flush, which discharges EITHER hold with one fit),
    src/components/terminal/resizeCoordinator.ts:364-371 (RO routes through `fitOrHold`).
    Pinned: `components/TerminalPane.test.tsx` "TerminalPane holds the PTY resize while a
    touch-scroll gesture is active": "performs NO local refit while the gesture holds the
    pair", "refits exactly once at the lift, together with the one send", "defers the
    foreground-resync resize (a direct send) to the lift"; the library fact by
    `src/lib/termresize.xterm.test.ts` (all three tests, alt buffer and width-only
    included).

12. **NEW at 39f5c2ce: while held, the FIRST direct resize closure wins; later ones are
    dropped.** Trap: plain resize sends are interchangeable (each re-reads live geometry
    when it runs) but the first-open jiggle closure is not; a later plain resize
    overwriting a parked jiggle silently skips the redraw nudge for that open
    (`initialResizeDone` is already latched). The jiggle's 60ms continuation is its own
    direct send and takes the same hold. Fix: src/components/terminal/resizeCoordinator.ts:156-157 (`heldResizeSend`),
    src/components/terminal/resizeCoordinator.ts:249-262 (first-one-wins in `fitAndSend`),
    src/components/terminal/resizeCoordinator.ts:305-311 (continuation through `fitAndSend`).
    Pinned: the held-resize suite above pins the hold and single flush; the
    first-one-wins ordering itself is NEW at the rebuild,
    `src/components/terminal/resizeCoordinator.test.ts` "keeps the FIRST held
    direct send and drops later ones" (and "takes the gesture hold for the
    jiggle's own 60ms continuation").

13. **The debounced `sendSize` also holds during a touch scroll and re-arms at the lift.**
    Trap: the debounced SIGWINCH coming due mid-wheel-stream corrupts the pager repaint
    (see A11); the keyboard-collapse resize is the ordinary trigger, not an exotic one.
    Fix: src/components/terminal/resizeCoordinator.ts:138-145 (`resizeHeldByGesture`), src/components/terminal/resizeCoordinator.ts:195-211 (`sendSize` holds, and hands its parked fit to the gesture), src/components/terminal/resizeCoordinator.ts:422-427 (the lift re-arms).
    Pinned: `components/TerminalPane.test.tsx` "defers the debounced resize until
    touchend, then sends exactly once", "touchcancel flushes a held resize too",
    "a gesture without any held resize sends nothing on touchend".

14. **The terminal opens synchronously against fallback metrics, then refits when the
    bundled fonts (and any user family) load.** Trap: awaiting fonts before `open()`
    delays the PTY connection on every mount for a benefit that only matters on a cold
    cache; a stale refit after remount must not touch a successor terminal (identity
    guard). Fix: src/components/terminal/useTerminalLifecycle.ts:397-405, `src/lib/terminalFont.ts:149-199`.
    Pinned: `components/TerminalPane.test.tsx` "TerminalPane bundled font load on mount"
    (both tests).

15. **Live font preference changes mutate the open terminal's options in place and
    refit; out-of-range sizes degrade to the default.** Verified against xterm 6.0.0:
    only `cols`/`rows` are read-only options. Fix: TP:359-380,
    `src/lib/terminalFont.ts:117-130`. Pinned: `components/TerminalPane.test.tsx`
    "TerminalPane live font preferences" (three tests); `src/lib/terminalFont.test.ts`
    `clampTerminalFontSize` suite.

16. **A user-supplied font family is sanitized (ASCII allowlist, 200-char cap) before it
    reaches xterm's inline `<style>` and `document.fonts.load`.** Trap: a raw `;`, brace,
    or newline in the value terminates or hijacks the CSS declaration. Non-ASCII names
    deliberately degrade to the bundled stack. Fix: `src/lib/terminalFont.ts:62-103`.
    Pinned: `src/lib/terminalFont.test.ts` "sanitizes a value that would otherwise break
    out of the CSS declaration" suite.

17. **Scrollback is sized from `agent_scrollback_lines` via a ref, read lazily on
    (re)connect.** Trap: xterm's 1000-line default trims the reconnect replay; making
    bootstrap a mount-effect dep would recreate the terminal. Fix: src/components/terminal/liveValues.ts:42-44 + TP:261, src/components/terminal/useTerminalLifecycle.ts:268.
    Pinned: unpinned.

18. **The xterm scrollbar width comes from the one `--xterm-scrollbar-width` CSS var.**
    Trap: the slimmed scrollbar and the button overlay's reserved gutter must agree
    (single source); setting `overviewRuler.width` also instantiates a ruler canvas that
    index.css hides. Fix: src/components/terminal/useTerminalLifecycle.ts:229-241, src/components/terminal/useTerminalLifecycle.ts:271-271. Pinned: unpinned.

19. **The terminal background is the app's `--background` token, resolved through a 1x1
    canvas paint (oklch to hex), applied to canvas and padded host alike.**
    Fix: src/components/terminal/useTerminalLifecycle.ts:203-224, src/components/terminal/useTerminalLifecycle.ts:272-272. Pinned: unpinned.

20. **NEW at the viewer-geometry arc: the PTY's grid is ON THE WIRE, twice over.**
    Trap (proven): one PTY has ONE authoritative grid, the owner's, and every other
    attached browser renders the same byte stream into its own differently sized
    xterm. Nothing told a non-owner the PTY's size (no field on the event bus, none
    on the pty-socket `connected` frame), so a viewer could not know that its live
    view was wrapped and clamped and that every child repaint was scrolling mangled
    rows into its LOCAL scrollback, where they stay until a fresh attach. The
    handshake now carries the grid at attach (`rows`/`cols`, always serialized, null
    when the server could not read the pty, which the client reads as "nothing
    known" and never as agreement), and every APPLIED resize is pushed to every
    socket attached to that pty as a Text event frame
    `{"event":"size","rows":R,"cols":C,"seq":N}`. A refused resize (a non-owner's
    plain frame) applies nothing and therefore announces nothing, exactly like the
    `pty.owner` broadcast beside it, or every viewer would heal itself towards a
    size the child never took. The broadcast is a `tokio::sync::broadcast` read by
    one more arm in each socket's existing `select!` loop, NOT a registry of sinks:
    the crate deliberately holds no other task's sink and locks none across an
    await.
    ORDERED BY `seq`, exactly the way `pty.owner` is ordered by `epoch`. Trap
    (found in review): the applies serialize under the owners lock, but each
    socket task publishes to the bus AFTER releasing it, so two sockets'
    announcements of two ordered applies can invert (A applies G2, B takes over
    and applies G3, B's publish lands first, A's stale G2 becomes every viewer's
    last word). `claim_for_resize` therefore stamps a per-pty seq inside the
    same critical section that enqueues the resize, the `size` frame carries it,
    and the client keeps only the highest seq seen, dropping any older arrival.
    The handshake carries the same counter as `grid_seq`, read server-side
    BEFORE the actor-queued grid read (so the handshake's grid is at least as
    new as it); the client seeds its filter from it, which is what stops a stale
    broadcast buffered from before the handshake from regressing the grid after
    the attach (and from arming a spurious heal bounce). The server's select arm
    keeps the same filter, seeded the same way, purely to save the wire trip;
    the client-side filter is the one that must exist. Old servers stamp no
    seqs, and the client then leaves the filter off rather than reading "no
    seq" as "seq zero".
    Fix: `crates/dux-core/src/pty.rs` (`grid_size`), `crates/dux-web/src/pty_sizes.rs`
    (the bus, and the two doc paragraphs on why it is a broadcast and not the
    event bus), `crates/dux-web/src/engine_actor.rs` (`PtyGridSize` /
    `pty_grid_size`), `crates/dux-web/src/pty_owners.rs` (`OwnersState.grid_seq`,
    the seq stamp in `claim_for_resize`, the `grid_seq` accessor),
    `crates/dux-web/src/server.rs` (`PtyConnectedFrame.rows/cols/grid_seq`,
    `PtySizeFrame`, `pty_size_frame_text`, the `grid_changes` select arm and its
    seq filter, the publish inside the `outcome.seq` branch),
    `src/lib/ptySocket.ts` (`onPtyGrid`, the `grid` getter, `readGrid`,
    `lastGridSeq`).
    RESIZE PATHS COVERED: for a web client the ONE path that resizes a live pty's
    grid is the socket's Text arm (`claim_for_resize` -> `EngineRequest::ResizePty`),
    which is where the publish sits. Launch and reopen paths SPAWN at their initial
    geometry rather than resizing a live pty, and a freshly spawned pty has no
    attached socket to tell (the first attach reads the grid off the handshake).
    The TUI's own `provider.resize` never coexists with web sockets, because
    `dux server` runs headless.
    Pinned: `crates/dux-core/src/pty.rs`
    `grid_size_reports_the_spawn_geometry_and_follows_every_resize`;
    `crates/dux-web/src/pty_sizes.rs` (both tests);
    `crates/dux-web/src/server.rs` `pty_connected_frame_serializes_with_generation`,
    `pty_connected_frame_spells_an_unknown_grid_as_explicit_nulls`,
    `pty_size_frame_serializes_as_a_size_event`;
    `crates/dux-web/tests/ws_transport.rs`
    `an_applied_resize_tells_every_attached_socket_the_ptys_new_grid` (announced to
    every socket, NOT announced on a refusal, and the handshake reporting the live
    grid); `src/lib/ptySocket.test.ts` "reports the pty's grid from the handshake
    and from every size event", "reads a grid the server could not answer as
    UNKNOWN, never as agreement", "drops a size event whose seq the handshake
    already covers", "drops a size event that arrives behind a newer one, by seq
    alone", and "applies size events without a seq, for an old server that
    stamps none"; `crates/dux-web/src/pty_owners.rs`
    `grid_seq_is_monotonic_per_pty_in_apply_order_and_absent_on_refusal`;
    `crates/dux-web/src/pty_sizes.rs`
    `a_stale_publish_arriving_after_a_newer_one_is_droppable_by_seq`.

21. **RETIRED at the sticky-demotion arc: the "sized for another device" badge is
    GONE, and the history is the point.** It was a small, quiet, click-through
    statement (`pointer-events-none`, no button, no menu) that a non-owner's local
    xterm grid differed from the wire-known PTY grid. The faithful view (A23) then
    removed the class of state it described: a watcher adopts the PTY's grid, so
    the two sides agree and the badge retired itself without knowing why. The only
    thing left standing it was the legacy `fit_window` view, and that preference is
    gone too (A25), so the element rendered for nobody.
    WHY THAT IS PROVABLE, and it is worth being careful about which argument is
    the real one. The tempting proof is "the full-pane take-over card covers every
    non-owner in every state", and it is FALSE: the connection-lost affordance
    stands the card down (C12) and raises a floating box, not a cover. The correct
    proof is the ADOPTION one: whenever a remote grid is known at all, faithful
    adoption has already made the grids equal, and no new remote grid can arrive
    on a dead socket. So the divergence the badge reports cannot come into being
    while the card is down.
    ACCEPTED RESIDUE, stated rather than hidden: a divergence frozen mid-gesture
    at the exact moment the socket dies (adoption in flight, the socket gone
    before it lands) is unlabeled for the duration of the connection-lost window.
    That window already carries its own, louder message, and Reconnect resolves
    both at once.
    WHAT STAYS: `gridsDiverge` (now the heal's own "is this announcement really a
    change" predicate, so the definition and the behavior cannot drift), the
    remote- and local-grid tracking, and the bounce-heal (A22). The heal is real
    behavior; only the pixel presentation was unreachable.
    Fix: `src/components/TerminalPane.tsx` (the badge JSX and
    `sizedForAnotherDevice`, both deleted),
    `src/components/terminal/viewerGrid.ts` (`gridsDiverge` rewired into
    `noteRemoteGrid`).
    Pinned: `src/components/terminal/viewerGrid.test.ts` `gridsDiverge` suite,
    which now guards the heal's change detection.

22. **NEW at the viewer-geometry arc: a viewer HEALS BY RE-ATTACHING, never by
    resizing the PTY.** A non-owner that hears a `size` event different from the last
    known remote grid schedules a socket bounce (`pty.connect()`), debounced by
    `VIEWER_HEAL_DEBOUNCE_MS` (500ms, chosen longer than both things that make
    applied grids arrive in bursts: the owner's own 200ms send debounce and the
    first open's two-step 60ms jiggle), so one gesture on the owner's desktop
    produces exactly ONE reconnect on a watching phone. The bounce is the existing
    reconnect machinery and nothing new (reset, fresh generation, replay, mode
    restore), which is precisely what clears the viewer-era scrollback; a viewer
    resizing the PTY instead would be the silent steal reborn. FIVE GUARDS, all
    mandatory: never when this client is the owner, never while a take-over intent
    is armed (take-over is already a bounce), never while a bounce is in flight,
    never without a socket (a dormant pane is never mounted), and the `connected`
    frame's own grid never triggers one (it would loop forever). At FIRING time the
    timer re-derives the four LIVE inputs (owner, take-over armed, bounce in
    flight, socket present) and routes them through the same
    `shouldHealByReattaching` table as arming, so the two decision points cannot
    drift; `fromHandshake` and `changed` are arming-time facts (a heal is only ever
    armed by a non-handshake change) and are passed as the constants that armed it.
    AMENDED at the faithful-view arc: the announcement is ADOPTED before it arms
    the heal (A23), which is an ordering requirement rather than an ordering
    accident: the bounce's replay must be parsed at the child's geometry, not at
    the one this window happened to have. The bounce itself is unchanged and
    still worth taking, because adopting the grid does not clean the scrollback
    the pre-adoption view already recorded; only a fresh attach does.
    A SOCKET OPEN CLEARS AN ARMED HEAL, not just the in-flight flag: any open (a
    network blip's reconnect, a take-over) has just rebuilt the buffer from the
    server's repaint, so a timer armed before it firing after it would be a
    redundant bounce at a just-healed socket. The reconnect cue is raised by hand,
    as the take-over bounce raises it, because a deliberate `connect()` fires no
    `onReconnecting`.
    RECORDED FOLLOW-UP: the `bouncing` latch is cleared by an open and by
    teardown, but not by the socket declaring the connection lost; a bounce whose
    socket never reopens leaves heals disabled for the rest of the mount (any
    later successful open, the manual Reconnect included, clears it). Wiring the
    connection-lost signal into the machine is deliberately not done here because
    it needs a new port through the lifecycle, not a one-line clear.
    Fix: `src/components/terminal/viewerGrid.ts` (`shouldHealByReattaching`, the
    debounce and the firing-time re-check), `src/components/terminal/constants.ts`
    (`VIEWER_HEAL_DEBOUNCE_MS`),
    `src/components/terminal/useTerminalLifecycle.ts` (`onPtyGrid`, the local-grid
    observation subscription, `noteSocketOpen` in `onOpen`).
    Pinned: `src/components/terminal/viewerGrid.test.ts` `shouldHealByReattaching`
    suite (one row per guard) and the `useViewerGrid` suite ("clears an armed heal
    when the socket opens...", "still bounces once when nothing intervenes...",
    "stands down at firing time when the client became the owner meanwhile");
    `components/TerminalPane.test.tsx` "TerminalPane
    viewer grid divergence": "bounces the socket ONCE after a burst of grid changes
    settles", "never bounces on the handshake's OWN grid", "never bounces the
    OWNER", "stands down while a take-over is armed".

23. **NEW at the faithful-view arc: a WATCHER RENDERS AT THE PTY'S TRUE GRID.**
    The divergence class is not managed, it is removed: the coordinator gains a
    VIEWER mode in which it never fits to the container and never sends, and
    re-grids this terminal to the rows and columns the wire reports instead
    (`term.resize`), so a watcher's emulator is geometry-identical to the
    driver's. The live view is then byte-faithful and the local scrollback can
    no longer record wrapped garbage.
    THE MODE IS NOT A LATCH, and since the sticky-demotion arc it has no
    preference behind it either: it is `viewerMode()`, which is exactly
    `!isOwner()` off the ownership verdict channel, so it cannot drift from who
    actually drives the pty, and neither promotion nor demotion needs a
    transition to be written: a take-over bounces the socket and its first frame
    fits and claims through the existing path (a blipped owner's self-succession
    is the same path, flagged against its own ghost; see C15), and a demotion is
    answered by the next `applyViewerGrid`. The
    owner's own applied grid is RECORDED in both modes precisely so a demotion
    has something to adopt at once rather than waiting for the next `size`
    event. Null is "nothing known", never agreement: the last grid the server
    did report stands.
    Fix: `src/components/terminal/resizeCoordinator.ts` (`viewerMode`,
    `onViewerLayout`, `runFit`, `applyViewerGrid`, `noteRemoteGrid`,
    `refitForFonts`), `src/components/terminal/useTerminalLifecycle.ts` (the
    coordinator deps, the adopt in `onPtyGrid`, the `viewerRegridRef` /
    `viewerRelayoutRef` ports).
    Pinned: `src/components/terminal/resizeCoordinator.test.ts` "the resize
    coordinator in VIEWER mode" (never fits, never sends, adopts on seed and on
    change, idempotent, null is not agreement, adopts on demotion, one fit+send
    on promotion);
    `components/TerminalPane.test.tsx` "TerminalPane viewer grid divergence":
    "adopts the PTY's grid rather than diverging from it", "adopts a grid CHANGE
    too", "never adopts anything for the OWNER", "adopts on DEMOTION".

24. **NEW at the faithful-view arc: the presentation is a FONT SHRINK, never a
    CSS transform.** The adopted grid is made to fit by choosing the largest
    half-pixel font size at which it does, from the cell measured at the current
    size (cell metrics are font-relative, so one measurement answers for every
    candidate). A `scale()` would have been one line and would have broken
    xterm's pixel-to-cell arithmetic everywhere it matters: selection, link
    resolution and the forwarded touch gestures all divide a client-space rect
    by the grid, and a transformed element reports the scaled rect while xterm's
    own hit-testing does not agree with it. The font is FLOORED at
    `VIEWER_MIN_FONT_SIZE` (7px, a legibility judgement, not a measurement);
    below the floor the terminal is left overflowing at its true size and the
    host becomes pannable, which keeps the picture correct where shrinking
    further would only make it illegible. The shrink never grows past the user's
    own `ui.terminal_font_size`. An UNMEASURABLE container answers with the
    user's size rather than the floor, so a pane that has not laid out yet does
    not stamp 7px text and bounce back a frame later.
    HOW THE OVERFLOW PANS, stated honestly. On a desktop the host's own
    scrollbars are the pan; the mouse wheel still goes to xterm and moves its
    scrollback, never the pan. On touch, while the host can actually scroll
    vertically, a vertical drag is left to the browser (the same treatment the
    no-forward alt-screen case gets), so the pan works but a drag cannot move
    xterm's scrollback in that state, and a watcher has no accessory keys to
    move it with either; that is the accepted cost of the minimal fix, and a
    width-only overflow keeps the drag on the scrollback. The gate is the
    pane's `viewerOverflow` flag through the live-settings container plus a
    live host measurement, in the gesture's `scrollAllowed`.
    Recomputed on exactly two things: the layout moving and the remote grid
    changing. The layout signal is the coordinator's own ResizeObserver
    calling `onViewerLayout` in place of the fit it does not run (one signal,
    never two observers), and that observer watches the HOST, not the
    container: the overflow branch pins the container to the grid's pixel
    size, and a pinned box never moves with the window, so observing it left
    the below-floor state deaf to resizes and stuck in pan mode. The host is
    never pinned, and the owner path is unaffected because the owner's
    container tracks the host exactly. It runs in a LAYOUT effect because it
    measures, and the pane's live font-preference effect was folded into it:
    one place decides what font the open terminal wears in both modes. The
    live-settings snapshot is published from a LAYOUT effect ordered before
    the relayout, so the relayout reads THIS commit's values rather than the
    previous one's. Leaving the faithful branch is its own case: a PROMOTION
    can change neither family nor size, so the relayout fits once on the
    transition itself, or the freshly promoted owner would stand at the grid it
    adopted as a watcher forever.
    Fix: `src/lib/viewerFit.ts` (`viewerFontFit`, `VIEWER_MIN_FONT_SIZE`,
    `VIEWER_FONT_STEP`), the relayout layout-effect in
    `src/components/TerminalPane.tsx`, `src/components/terminal/constants.ts`
    (`xtermScrollbarWidth`, shared with the lifecycle so the gutter is measured
    once), `src/lib/terminalFont.ts` (`loadTerminalFontsThenRefit` now takes the
    refit as a closure, because that refit has two right answers).
    Pinned: `src/lib/viewerFit.test.ts` (whole file: exact fit, never grows,
    steps down, tighter axis wins, floor clamp with the overflow flag, a
    preference below the floor, and every degenerate measurement);
    `components/TerminalPane.test.tsx` "TerminalPane faithful-view overflow
    and live preference flips" (the observer watches the host and a growth
    un-pins the overflow, a promotion out of the faithful branch fits once
    immediately, a vertical drag is left to the browser while the overflow
    scrolls vertically and stays intercepted otherwise); `src/components/terminal/constants.test.ts`
    (an explicit 0 scrollbar width is honored, only unset falls back to 8).

25. **RETIRED at the sticky-demotion arc: `ui.watcher_view` is GONE, and a
    watcher always renders faithfully.** For one unreleased arc it was a real
    preference with two values, `faithful` (the default) and `fit_window` (the
    pre-faithful behavior, with the badge and the polluted scrollback), riding
    the generic settings machinery end to end. It is removed because the
    full-pane take-over card (C17) covers a watcher's terminal in every ordinary
    state, so the only difference the two modes produced was hidden behind it,
    and the faithful buffer is strictly better in the states that are visible: a
    clean scrollback and an instantly clean take-over. Keeping a setting whose
    effect nobody can see is a way to accumulate untested states.
    NEVER SHIPPED, so nothing migrates: the key was added on `server-mode` and is
    not in `main` (the check-shipped rule). `UiConfig` has no
    `deny_unknown_fields`, so a config file still carrying a `watcher_view` line
    loads exactly as it did and the key is ignored; `toml_edit` saves leave the
    stale line in place, and `dux config regenerate` is the way to tidy it away.
    Removed from: `crates/dux-core/src/config.rs` (`WatcherViewMode`,
    `UiConfig::watcher_view`, `watcher_view_load_warning` and its load site),
    `crates/dux-core/src/config_write.rs`, `crates/dux-core/src/viewmodel.rs`
    (`BootstrapView::watcher_view` and its projection),
    `crates/dux-core/src/wire.rs` (`SettingsPatch::watcher_view`, its validation
    and its settings row, count 23 down to 22),
    `crates/dux-web/src/config_routes.rs`, `crates/dux-tui/src/config.rs` (the
    canonical commented template), `src/lib/bootstrapApi.ts`,
    `src/lib/settingsDescriptors.ts`, `src/lib/viewerFit.ts` (`WatcherView`,
    `watcherViewMode`), `src/components/terminal/liveValues.ts`
    (`watcherFaithful`), the coordinator's `viewerMode` dep and the pane's
    `faithfulWatcher` derivation.
    Pinned: `crates/dux-core/src/config.rs`
    `a_leftover_watcher_view_line_is_ignored_rather_than_failing_the_load`;
    `crates/dux-core/src/wire.rs` `set_settings_applies_every_field_when_sent_alone`
    (the row count); `src/lib/settingsDescriptors.test.ts` (the exposed key set
    and the cross-language PATCH key pin);
    `components/CustomizeWebappDialog.test.tsx` "renders a select for enum
    settings" (three rows, not four).

## B. Attach, replay, freshness

1. **Opening the PTY socket IS the subscription; connecting an agent socket
    launches/resumes the provider**, and since the take-over arc, `pty.connect()` is
    also how a take-over is performed, so the bounce re-subscribes rather than opening
    anything new (the pty is already live; a second connection to it is an ordinary
    viewer attach, and the server replays it the scrollback unconditionally). A dormant tab must never be auto-mounted, because
    subscribing force-launches (App renders its card instead; reaching this pane for one
    is an intentional launch). Fix: src/components/terminal/useTerminalLifecycle.ts:334-338 (the socket per target), src/components/terminal/useTerminalLifecycle.ts:962-964 (`pty.connect()`); `crates/dux-web/src/server.rs:1441-1449`.
    Pinned: not by this pane's suites (App-level dormant-tab tests plus the CLAUDE.md
    tab tenet).

2. **The server's first frame is Text `connected` carrying this socket's connection id
    and the replay generation; the replay follows as one Binary frame.**
    AMENDED at the take-over arc (it also names the pty's `owner` and the
    `owner_epoch` of that snapshot, C1), again at the viewer-geometry arc (it
    also carries the pty's `rows`/`cols`, plus the `grid_seq` those are at least
    as new as, A20), and again when it gained the owner's DEVICE label
    (`owner_device`, the owner's captured `User-Agent`, read under the same
    owners-lock acquisition as `owner` and omitted when there is none to name;
    C9). It is now the only frame a client needs to know everything about the
    pty it just joined: who drives it, since when, from what device, and at
    what geometry. `size` is the one OTHER Text frame the server sends on this
    socket, and the two are told apart by `event` alone.
    Fix: `src/lib/ptySocket.ts` (`handleMessage`); `crates/dux-web/src/server.rs`
    (`PtyConnectedFrame`, `send_pty_connected`).
    Pinned: `src/lib/ptySocket.test.ts` "records connection id and replay generation from
    the connected frame", "leaves replayGeneration null when the connected frame omits gen",
    "passes the handshake's owner_device through, and undefined when absent";
    `crates/dux-web/src/server.rs`
    `pty_connected_frame_names_the_owners_device_and_omits_an_absent_one`.

3. **Replay idempotency by generation (Mechanism A): a replay whose generation was
    already applied is dropped whole, no reset, no write.** Trap: on mobile the socket
    reconnects constantly; a duplicate replay or a late blob from a torn-down forwarder
    stacked a second copy of history (the duplicated-text bug). Untagged (older server)
    always applies. Fix: src/components/terminal/attachReplay.ts:151-159, `src/lib/replayGeneration.ts`;
    `crates/dux-web/src/server.rs:1473-1482,1808`.
    Pinned: `src/lib/replayGeneration.test.ts` (whole file);
    `crates/dux-web/src/server.rs` `replay_generations_are_strictly_increasing`.

4. **A reconnect resets xterm before applying the replay, and the reset is drain-gated:
    the previous connection's write queue drains first, bytes arriving mid-drain are held
    and flushed in order after the reset.** Trap: writing the replay over the pre-drop
    buffer stacks history; a stale queued byte landing after `reset()` corrupts the fresh
    replay; reordering is the failure mode either way. Fix: src/components/terminal/attachReplay.ts:85-185 (`draining`,
    `heldChunks`, empty-write callback). Pinned: the drain-path focus-report test
    (I3), `src/lib/ptySocket.test.ts` "reconnects after an unexpected close and
    receives the replay (resends nothing)", and NEW at the rebuild
    `src/components/terminal/attachReplay.test.ts` "drains, resets, and only then
    writes the replay" and "HOLDS bytes that race in mid-drain and writes them in
    order after the reset", which pin the held-chunk ORDERING itself.

5. **`reset()` clears private modes the child set once at startup and never repeats, so
    the server's repaint carries an explicit mode-restore tail.** Trap: without it a
    reconnect landed on a full-screen agent with `mouseTrackingMode === "none"` and a
    finger drag did nothing until a hard refresh. Do not infer modes client-side from
    what the replay draws. Fix: src/components/terminal/attachReplay.ts:13-21;
    `crates/dux-core/src/pty.rs:234-301` (`mode_restore_sequence`),
    `crates/dux-core/src/pty.rs:2197-2222` (`reconnect_repaint`).
    Pinned: `crates/dux-core/src/pty.rs` `mode_restore_sequence_emits_both_polarities`,
    `reconnect_repaint_restores_private_modes_on_the_alt_screen` /
    `_on_the_main_screen`, `reconnect_repaint_restores_default_modes_when_the_child_set_none`.

6. **The mode restore emits tracking-mode disables before the enable, ascending.**
    Trap (measured): xterm keeps one active protocol and a DECRST of ANY of
    1000/1002/1003 drops it to none, so `1000l 1002h 1003l` leaves tracking off.
    Fix: `crates/dux-core/src/pty.rs:244-275`.
    Pinned: `crates/dux-core/src/pty.rs`
    `mode_restore_sequence_enables_mouse_tracking_after_its_disables`.

7. **The repaint restores the scroll region and positions even a hidden cursor; origin
    mode is cleared, not restored.** Fix: `crates/dux-core/src/pty.rs:2197-2259` and the
    doc at 218-233. Pinned: `crates/dux-core/src/pty.rs`
    `reconnect_repaint_restores_the_scroll_region_on_the_alt_screen` / `_main_screen`,
    `reconnect_repaint_places_a_hidden_cursor_where_the_program_left_it`,
    `reconnect_repaint_clears_origin_mode_before_it_positions_anything`.

8. **`onOpen` clears the stale connection id immediately (and retires it from the
    own-connection set), not just on the next `connected` frame.** Load-bearing for the
    take-over bounce too: the reopen mints a fresh id, and the armed intent is what keeps
    the pane an owner across the gap in which it has none. Trap: a `pty.owner`
    over the separate `/ws/events` socket can arrive before this socket's new
    `connected` frame; a stale id makes `isOwnerAfterHandover` misjudge ownership. Null
    reads safely as non-owner until the new frame lands. Fix: src/components/terminal/useTerminalLifecycle.ts:895-910 + src/components/terminal/attachReplay.ts:134-139.
    Pinned: `components/TerminalPane.test.tsx` "registers its socket id as this client's
    own and retires it on reconnect and unmount".

9. **`onReconnecting` shows a non-blocking overlay and also retires the dead id, and
    since the take-over arc the take-over raises that same overlay by hand.**
    Trap: input typed while disconnected is silently dropped by the readyState guard, and
    the overlay is the only signal that it would be. A DELIBERATE `connect()` fires no
    `onReconnecting` at all (`ReconnectingSocket.connect` goes straight to `open()`), so
    without the hand-raised cue the take-over's half-second bounce would read as a dead
    terminal. A take-over in a genuine reconnect window needs no special case any more:
    it arms the intent and bounces like every other press.
    Fix: `TerminalPane.tsx` (the `reconnecting` state, hoisted above the ownership
    machine so `takeOver` can raise it), src/components/terminal/ownership.ts
    (`setReconnecting(true)` in `takeOver`),
    src/components/terminal/useTerminalLifecycle.ts (`onOpen`/`onReconnecting`). Pinned: `src/lib/ptySocket.test.ts` "fires
    onReconnecting once when the socket drops, then onOpen on recovery", "does not fire
    onReconnecting on a user-initiated close".

10. **The PTY socket shares the events socket's 3-attempt reconnect cap; `failed` is a
    hard stop with an explicit Reconnect affordance.** Trap: before the cap, a dead
    server silently reattached behind a stuck overlay. The affordance suppresses itself
    while the whole app is offline (the OfflineOverlay owns that signal). Fix:
    src/components/terminal/ownership.ts:213-217, src/components/terminal/useTerminalLifecycle.ts:941-948, TP:834-857.
    Pinned: `components/TerminalPane.test.tsx` "TerminalPane connectionLost affordance"
    (describe.each over agent AND companion terminal), incl. "suppresses its own
    connectionLost overlay while globally offline".

11. **Reconnect reconnects THIS pane's own socket (`pty.connect()`), never an epoch
    bump.** Trap: `terminalEpoch` only feeds the pane key for agents; an epoch bump is a
    no-op for a companion terminal and leaves its Reconnect button dead. Fix:
    TP:835-851. Pinned: `components/TerminalPane.test.tsx` "Reconnect calls the pane's
    OWN socket.connect() (not an epoch no-op)".

12. **Server close code 4001 (provider unavailable/exited) stops the client cold.**
    Trap: auto-retry re-subscribes and relaunches the doomed provider forever.
    Fix: `src/lib/ptySocket.ts:36-42,270-288`; `crates/dux-web/src/server.rs:829-835,
    1455-1471,1538-1556`. Pinned: `crates/dux-web/src/server.rs` tests asserting
    `PROVIDER_GONE_CLOSE_CODE` (lines ~2635, ~2647).

13. **An extra tab's vanished route is detected by consulting the spine, not the close.**
    Trap: a closed WebSocket carries no HTTP status, so a 404-forever route is
    indistinguishable from a transient drop and would retry forever. Session-slot tabs
    and companion terminals never need this. Fix: src/components/terminal/useTerminalLifecycle.ts:949-961 (`shouldRetry`/`onGone`),
    `src/lib/ptySocket.ts:178-193`, `src/lib/agentTabs.ts:36`.
    Pinned: `src/lib/agentTabs.test.ts` "isTabGone" suite; the pane wiring itself is
    unpinned.

14. **`everReady` latches on first output and never un-latches.** Trap: an exited agent
    reports `has_output: false` again; without the latch the startup spinner returns
    over a pane full of output. Fix: TP:503-510 (render-phase latch, guarded).
    Pinned: `components/TerminalPane.test.tsx` "shows the launch spinner while the
    project terminal has no output yet" (spinner half); the latch itself unpinned.

15. **A session-slot tab's exit (agent leaves `active`) ejects to the welcome screen,
    marked as dux's own eject; an extra tab's exit does not.** Fix: TP:660-674
    (`ejectSelectionForReconnect`, gated on `isSessionSlotTab`). Pinned: not in this
    pane's suites (store/App-level).

16. **Binary frames are the byte stream; the replay is just the first of them; nothing
    is resent by the client on reopen.** Fix: `src/lib/ptySocket.ts:223-235`.
    Pinned: `src/lib/ptySocket.test.ts` "streams server Binary frames to onBytes as
    Uint8Array", "reconnects ... (resends nothing)".

## C. Ownership

1. **REWRITTEN at the take-over arc: ATTACHING NEVER STEALS. The `connected` handshake
    says who drives, and the foreground guess survives only for claiming an UNOWNED
    pty.** Trap (measured in the container, pre-fix): a foregrounded page ATTACHING
    claimed unconditionally (owner conn 8 to 9 on mere attach), even from a live owner,
    so every phone-open of a desktop-driven agent was a silent SIGWINCH tug-of-war and
    the take-over card only appeared because the focused desktop re-took it. Half the fix
    is server-side (C2); the other half is that a refused claim emits NOTHING, so without
    a handshake answer the client's optimistic guess would wedge it as a phantom owner:
    typing surfaces up, every keystroke dropped, no card ever. No-document contexts still
    read as foreground so a claim is never silently suppressed.
    Fix: src/components/terminal/ownership.ts (site 1 and site 4, `seedFromConnected`),
    `src/lib/ptyOwnership.ts` (`seedVerdictFromConnected`, `isForeground`),
    `src/lib/ptySocket.ts` (the three-valued `handshakeOwner`);
    server side `crates/dux-web/src/server.rs` (`current_owner` read + `PtyConnectedFrame.owner`).
    Pinned: `src/lib/ptyOwnership.test.ts` "isForeground" and "seedVerdictFromConnected"
    suites; `src/lib/ptySocket.test.ts` "tells an absent owner key apart from an
    explicitly null one"; `src/components/terminal/ownership.test.ts` "seeding the
    verdict from the connected handshake" suite; `components/TerminalPane.test.tsx`
    "TerminalPane seeds its ownership verdict from the connected frame" suite;
    `crates/dux-web/tests/ws_transport.rs`
    `the_connected_handshake_names_the_ptys_current_owner`.

2. **REWRITTEN at the take-over arc: server-side, a resize claims only an UNOWNED pty
    or one whose frame explicitly says `takeover`; every other non-owner resize is
    refused WHOLE (nothing applied, nothing broadcast, logged at debug).** The first
    Binary writer of an UNOWNED PTY still claims. All of it resolves in ONE critical
    section: `claim_for_resize` takes the owners lock, decides, and ENQUEUES the engine
    resize under the same lock. Trap (verified pre-fix): claims serialize on the owners
    mutex while `engine.resize_pty` is a separate `try_send` from a per-socket tokio
    task, so nothing bound the two orders and claim A then B could leave the pty sized
    for A with B recorded as its driver, permanently (B believes it already told the
    child). Two doc comments claiming a non-owner's resize was already ignored were
    fiction and are corrected in the same change.
    AMENDED at the viewer-geometry arc: the same branch that decides `apply` is
    now also where an APPLIED grid is announced to every socket attached to the
    pty (A20). The two halves are deliberately one condition, so a refusal can
    never announce: `outcome.apply` gates the publish exactly as `outcome.epoch`
    gates the `pty.owner` emit beside it, and both run after the owners lock is
    released.
    Fix: `crates/dux-web/src/pty_owners.rs` (`claim_for_resize`, `ResizeClaim`),
    `crates/dux-web/src/server.rs` (the Text-frame arm, `PtyResizeFrame.takeover`,
    the `pty_grid_bus.publish` inside the `apply` branch).
    Pinned: `crates/dux-web/src/pty_owners.rs` `claim_for_resize_table` (the full
    {unowned, owned-by-other, owned-by-self} x {plain, takeover} table) and
    `claim_for_resize_applies_in_claim_order_so_the_owner_owns_the_geometry`;
    `crates/dux-web/tests/ws_transport.rs`
    `a_second_pty_connection_is_replayed_scrollback_and_claims_only_by_taking_over`
    (re-scoped: it used to pin the steal); the remaining `PtySizeOwners` tests in
    `crates/dux-web/src/server.rs`.

3. **Ownership after a handover is a definitive id comparison, never a timing/echo
    heuristic.** Trap: the old echo-counting guess inverted when two devices claimed in
    the same instant and broadcast order flipped, leaving BOTH on the placeholder.
    Missing id on either side reads as "not us". Fix: src/components/terminal/ownership.ts:248-267,
    `src/lib/ptyOwnership.ts:127-132`. Pinned: `src/lib/ptyOwnership.test.ts`
    "isOwnerAfterHandover" suite and "drives the ownership decision end to end (own
    claim vs foreign takeover)".

4. **`pty.owner` events are epoch-deduped per pty id.** Trap: the epoch is assigned
    under the server lock but broadcast after it releases; the runtime can reorder two
    near-simultaneous broadcasts and a stale owner would win. Absent epoch (mixed
    versions) always delivers. Fix: `src/lib/ptyOwnership.ts:154-201`.
    Pinned: `src/lib/ptyOwnership.test.ts` "pty.owner epoch dedup" suite.

5. **The epoch high-water marks reset when the events socket reconnects.** Trap: the
    server's counter restarts at zero on restart; a client holding a high mark would
    drop every post-restart handover as stale. Fix: `src/lib/ptyOwnership.ts:163-171`.
    Pinned: `src/lib/ptyOwnership.test.ts` "resetPtyOwnerEpochs clears high-water marks
    so a post-restart epoch is not dropped".

6. **Input is owner-gated two-deep: `onData`/`onBinary` return early client-side, and
    the server's `may_write` drops a non-owner's stdin too.** SIZES are gated the same
    way and the gate now matters more: a non-owner NEVER volunteers a size (the
    coordinator's `isOwner` gate), because against an old server a volunteered non-owner
    size is the silent steal reborn, and against a new one it is refused-resize log spam
    on every alt-tab. The flagged take-over claim is the ONLY frame this client sends
    while it knows it is not the owner. Fix: src/components/terminal/useTerminalLifecycle.ts:440-444,
    src/components/terminal/useTerminalLifecycle.ts:470-473,
    src/components/terminal/resizeCoordinator.ts (`sendOwned`);
    `crates/dux-web/src/server.rs` (the Binary and Text arms).
    Pinned: `src/lib/termkeys.test.ts` softNewlineAction owner tests;
    `components/TerminalPane.clipboard.test.tsx` "still keeps a viewer's TEXT paste off
    the wire"; server `PtySizeOwners` tests.

7. **`isOwnerRef` flips synchronously at the mutation points, never during render.**
    Trap: an in-flight keystroke must be gated by the new state at once, before the
    re-render lands. Fix: src/components/terminal/ownership.ts:165-176 (the verdict
    CHANNEL, whose write flips the synchronous read and the rendered state
    together), src/components/terminal/ownership.ts:264-267, :338-345.
    The handshake seed (C1) and the freed-pty claim write through the same channel.
    Pinned: the handover suites, and NEW at the rebuild
    `src/components/terminal/ownership.test.ts` "demotes this client when the
    claimer's id is somebody else's", which asserts the channel read directly.

8. **REWRITTEN at the take-over arc: TAKE-OVER IS A FRESH ATTACH.** `takeOver()` arms an
    intent, flips the verdict, raises the reconnect cue, and calls `pty.connect()`. It
    sends nothing down the live socket; the claim rides the reconnect's ordinary
    first-frame resize, flagged, so ownership lags the press by one reconnect and one
    replay parse. Trap (proven in the container): a live claim resized the PTY and the
    child repainted cleanly, but the VIEWER's own scrollback was the polluted thing (a
    narrow viewer records mangled wrapped rows from every wide-owner repaint) and nothing
    cleared it, so scrolling up after a take-over read back garbage. A reconnect runs the
    reset-then-repaint path, which does. The old dead-socket special case collapses into
    this: `connect()` refills a spent budget and detaches a live socket, so there is one
    path, not two. `connect()` fires no `onReconnecting`, so the cue is raised by hand or
    the half-second window reads as a frozen terminal. Idempotent while armed. Accepted
    cost: every take-over is a reset + replay + SIGWINCH rather than one Text frame.
    Fix: src/components/terminal/ownership.ts (`takeOver`, site 3),
    src/components/terminal/channels.ts (`TakeoverIntent`),
    src/components/terminal/useTerminalLifecycle.ts (the `sendResize` wrapper that
    consumes it, and the teardown that clears it).
    Pinned: `components/TerminalPane.test.tsx` "TerminalPane take-over is a fresh attach"
    (all five tests, including the reset-and-replay one and the dropped-frame retry);
    `src/components/terminal/ownership.test.ts` "taking over" suite (arm, idempotence
    mid-bounce, survival across the bounce, retirement on demotion).

9. **The take-over card names the other device from the owner's `User-Agent`, which
    reaches a watcher on TWO frames: the `pty.owner` handover's `device`, and the
    `connected` handshake's `owner_device`. The specific name is dropped whenever the
    events socket is not open.** The handshake half exists because a mere attach
    hears no `pty.owner` at all under attach-never-steals (the regression a user
    reported: every watcher's card had degraded to "Active on another device"),
    so the server records the UA in the owner map at claim time and the seed
    stores it exactly as a handover would, through the pure
    `seedDeviceFromConnected` rule (owner pane never names a device, a
    SUPERSEDED handshake keeps the newer applied handover's name, an old
    server's absent key falls back generic). Trap: `pty.owner` is delivered
    live-only with no replay; across an outage the name goes stale, and the
    generic copy is never wrong. So the name is dropped on the events socket
    closing (the render-phase transition, not an effect) AND the handshake
    seed's name half is gated on that socket being OPEN: a handshake landing
    during an events outage seeds the verdict but not the name, because the
    broadcast channel that could later correct a name is exactly what is down.
    Fix: src/components/terminal/ownership.ts (`takeoverDevice`, the site-6
    render-phase clear, `seedFromConnected`, `deviceLabel` in the return);
    `src/lib/ptyOwnership.ts` (`seedDeviceFromConnected`); server
    `crates/dux-web/src/server.rs` (`captured_user_agent`,
    `PtyConnectedFrame.owner_device`) and `crates/dux-web/src/pty_owners.rs`
    (`OwnerRecord.device`, `current_owner`).
    Pinned: `components/TerminalPane.test.tsx` "TerminalPane take-over device naming"
    (all four tests, including "names the owning device from the connected handshake
    alone, no pty.owner event"); `src/lib/deviceLabel.test.ts`;
    `src/lib/ptyOwnership.test.ts` "seedDeviceFromConnected" suite;
    `crates/dux-web/src/pty_owners.rs` `current_owner_reports_the_device_recorded_at_claim_time`;
    `crates/dux-web/tests/ws_transport.rs` `the_connected_handshake_names_the_owners_device`.

10. **The pane publishes its ownership verdict into the store ledger, agent PTYs only,
    and a pane whose socket has failed for good publishes no verdict at all.** Trap: a
    stale "mine" from a dead connection would override the server's spine field forever
    on a surface that cannot type; a companion terminal taken over says nothing about
    the agent. The verdict it publishes is now the handshake-seeded one, so a phantom
    owner can no longer un-gate the agent menu either.
    Fix: src/components/terminal/ownership.ts (the ledger effect). Pinned: `components/TerminalPane.test.tsx`
    "TerminalPane ownership reporting into the store" (all four tests);
    `src/lib/storePtyOwnership.test.ts` (ledger and `sessionActiveElsewhere` suites).

11. **Own-connection ids are registered on `connected` and retired on reconnect, drop,
    and unmount, in step with `myConnIdRef`.** Trap: the server releases everything the
    id owned the moment the socket closes; a kept id makes a spine field naming it read
    as "mine" when it no longer is; the two "is this id mine" trackers must never
    disagree. Fix: src/components/terminal/useTerminalLifecycle.ts:410-415, src/components/terminal/useTerminalLifecycle.ts:905-909, src/components/terminal/useTerminalLifecycle.ts:930-934, src/components/terminal/useTerminalLifecycle.ts:1085-1089.
    Pinned: `components/TerminalPane.test.tsx` "registers its socket id as this client's
    own and retires it on reconnect and unmount"; `src/lib/storePtyOwnership.test.ts`
    "noteOwnPtyConnection registers and retires this client's socket ids".

12. **The take-over surface yields to the connection-lost affordance.** Trap: the
    card paints solid over the whole pane; a non-owner with a dead socket would see
    only "Take over" and never Reconnect. One state on screen at a time, by suppressing
    it rather than lifting z-orders: a watcher whose socket has died needs
    Reconnect, and a Take over that cannot reach the server is an offer dux cannot
    keep. Fix: the card's `!isOwner && !(connectionLost && !offline)` gate in
    `src/components/TerminalPane.tsx`.
    Pinned: `components/TerminalPane.test.tsx` "shows the Reconnect affordance, not the
    take-over card, on a dead socket".

13. **Typing surfaces render only for the owner; the input ⋯ menu is deliberately NOT
    owner-gated (view toggles are not input).** Trap: a viewer who hid the phone's top
    bar had hidden the menu with it and had no way back at all. Fix: TP:455-471,
    TP:968-975. Pinned: `components/TerminalPane.test.tsx` "hides the compose bar AND
    the accessory bar for a non-owner viewer" and "TerminalPane input menu for a
    non-owner" suite.

14. **REWRITTEN at the take-over arc: EVERY send now goes through the coordinator, claims
    included, and the coordinator has no send exception left.** Both claim paths flip the
    verdict to "mine" BEFORE they send (take-over before bouncing the socket, the
    freed-pty claim before asking the coordinator), so both pass the owner gate and are
    recorded like any other send. That is a change from the shape this entry used to
    describe, where a claim ran while `isOwnerRef` still said somebody else owned the pty
    and had to bypass the record. Steady-state resizes by the current owner still arm no
    handover. The surviving error direction is unchanged and still the safe one: the
    record books "written to the socket" and says nothing about what the server did with
    it, so a refused frame may be booked as sent and the foreground resync's forced
    re-send is the standing recovery (a same-size frame is a kernel no-op).
    Fix: src/components/terminal/resizeCoordinator.ts (`sendOwned` and the module doc,
    whose take-over exception is deleted).
    Pinned: A5's tests plus the take-over suite, and
    `src/components/terminal/resizeCoordinator.test.ts` "does not record a resize
    the OWNER GATE dropped...".

15. **REWRITTEN at the sticky-demotion arc: the owner's disconnect is BROADCAST, it
    only RE-TITLES the card, and a blipped owner takes its own pty back by
    SELF-SUCCESSION.** Trap: with the silent steal gone, nothing else ever corrects a
    departed owner. Before, the next device to attach or alt-tab took the pty and that
    theft was what cleared the stale card; now "Active on another device" would be a
    permanent lie about a browser tab that closed. So `release()` reports an epoch and
    the handler emits an owner-cleared `pty.owner` (no `owner` field, which every client
    reads as "not me"). The release takes a real epoch, not just a generation bump, or
    the client's epoch dedup discards the event as a stale duplicate and the lie
    survives anyway.
    NOBODY CLAIMS ON THAT BROADCAST. Every client demotes and every card re-titles
    itself to "Nobody is driving", foregrounded or not. LOSING OWNERSHIP IS STICKY until
    a deliberate act, and sitting on an open card is not one. The passive claim this
    entry used to describe (a mounted foregrounded viewer taking the freed pty through
    the coordinator's `directSend` port) is DELETED, along with the port, because it
    lost the wifi-blip race for the real owner: the server's liveness reap is send-
    failure based and lands tens of seconds after the drop, while the blipped client is
    back in about one, so an idle desktop sitting on a card won a race the returning
    driver did not know it was in.
    THE FOUR RE-CLAIM GESTURES are a full reload, the card's Take over button,
    navigating away to another agent and back, and the blipped owner's own reconnect.
    All four are a FRESH HANDSHAKE (or the explicit flagged claim from the button), and
    the last one needs its own rule, because the same reap lag means the handshake
    usually still names the returning client's OWN dead connection id rather than null.
    SELF-SUCCESSION covers it: the ownership machine keeps the pane's previous
    connection id (`prevConnIdRef`, written by the `connId` channel wherever the
    lifecycle nulls the live id), and a handshake whose owner equals that ghost, on a
    FOREGROUNDED page, arms the take-over intent so the claim rides the first resize
    frame of the new connection FLAGGED. The server grants a flagged claim against any
    owner and the owner being displaced is this pane's own ghost, so nothing is stolen.
    A superseded handshake does not self-succeed (rule 2 of the seed: another device's
    newer claim is already applied), and the late reap broadcasts nothing at all,
    because by then `release` finds a different owner recorded.
    DELIBERATE CONSEQUENCE, not a bug: an owner whose socket drops while its own tab is
    BACKGROUNDED does not self-succeed either. Its reconnect's handshake reseeds it as a
    watcher and its human presses Take over on return. That is "attaching never steals"
    applied to our own reconnect, which is an attach like any other.
    THE FREED EXCEPTION: an owner-cleared event leaves an ARMED take-over intent in
    place, because it names no winner and so clears nobody's victory; only an event
    naming ANOTHER owner (a genuine lost race) retires the intent.
    Fix: `crates/dux-web/src/pty_owners.rs` (`release` returning the epoch),
    `crates/dux-web/src/server.rs` (`pty_owner_cleared_event`, the disconnect path),
    src/components/terminal/ownership.ts (site 5 inside the `onPtyOwner` effect,
    `prevConnIdRef`, the self-succession branch of `seedFromConnected`),
    `src/components/TerminalPane.tsx` (the card's three titles).
    Pinned: `crates/dux-web/src/pty_owners.rs`
    `release_reports_an_epoch_only_when_it_cleared_a_real_owner`;
    `crates/dux-web/tests/ws_transport.rs`
    `an_owner_disconnecting_broadcasts_an_owner_cleared_pty_owner`;
    `src/components/terminal/ownership.test.ts` "a freed pty" suite (the absence of the
    claim, foregrounded and backgrounded, the demotion of a pane that believed it
    owned the pty, the armed intent surviving the freed broadcast so the next unowned
    handshake claims flagged, and the lost-race clear on an event naming another
    owner) and its "self-succession after a blipped socket" suite (claims back,
    not while backgrounded, not for somebody else's id, not against a superseded
    handshake); `components/TerminalPane.test.tsx` "says nobody is driving once the
    owner disconnects, to a backgrounded viewer", "does not claim the freed pty, even
    foregrounded and mounted", and "self-succeeds when the reconnect's handshake names
    its own dead connection".

16. **NEW at the take-over arc: ownership stops following focus, and mixed versions
    degrade one way.** A desktop taken over by the phone stays a WATCHER when refocused
    and shows the card until its human presses Take over. That matches the card's own
    copy ("Only one device can type at a time") and costs one tap when switching devices;
    what it buys is the death of the silent steal and the ping-pong. Stated honestly
    rather than defended as a refocus feature: the "instant re-claim" that used to make
    the old behavior look intentional was the optimistic-belief window plus the
    attach-steal, not a designed re-claim.
    MIXED VERSIONS: `takeover` has no `deny_unknown_fields` to trip over and cannot
    collide with the viewed-ping parse, and an unflagged frame is byte-identical to what
    every prior client sent, so an old CLIENT keeps working in the only case its claim
    was ever legitimately granted (an unowned pty). Its Take over button dies SILENTLY
    against a new server; the mitigation is the run-identity hard reload, which replaces
    a stale page as soon as the server run changes, bounding the window to one server
    run. Do not repeat the review-debunked "only unowned claims were legitimate" line as
    if it excused the button.
    Fix: `crates/dux-web/src/server.rs` (`PtyResizeFrame` doc), the CLAUDE.md
    web-terminal tenet, `website/docs/web-workspace.md`,
    `website/src/components/FeatureGrid.astro`.
    Pinned: unpinned by construction (it is the ABSENCE of a re-claim); the nearest
    guards are C2's claim table and
    `a_second_pty_connection_is_replayed_scrollback_and_claims_only_by_taking_over`.

17. **RESTORED after the faithful-view arc: the take-over card is FULL-PANE, and
    that is a design decision, not a leftover.** The faithful-view arc briefly
    turned it into a compact bottom banner, on the theory that the card had only
    ever been a shield over a garbage picture and that the now-clean picture
    underneath deserved to be seen. That theory misread what the card is FOR:
    it is deliberate communication that a device with a DIFFERENT viewport size
    is driving this PTY, and that taking over retargets the PTY's size to this
    device. A solid `bg-background` overlay across the whole pane says that
    plainly; a strip along one edge reads as a footnote to a screen that looks
    drivable. The card is not a rendering shield, and the faithful at-grid
    buffer (A23) does its work underneath it regardless: the watcher's local
    scrollback stays clean whatever covers the pane, and take-over remains a
    fresh attach (C8). Same three titles, same second description sentence
    ("Take over to drive this agent from here", or its terminal variant), same
    single confirm-free Take over action. The specific-name title ("Open on
    Chrome on macOS") is reachable by a mere attach too, seeded from the
    handshake's `owner_device` (C9), so a watcher that simply opened the pane
    is not stuck on the generic "Active on another device".
    Fix: `src/components/TerminalPane.tsx` (the `Card` overlay; the badge that
    used to sit under its z-20 is gone entirely, A21).
    Pinned: `components/TerminalPane.test.tsx` "TerminalPane take-over card"
    (full-pane solid backdrop with the terminal still mounted underneath, the
    three titles, the second sentence and the one action, and nothing at all
    for the owner), plus every pre-existing take-over test, which asserts the
    same titles and the same button.

## D. Keys and clipboard

1. **Shift-Enter is a soft newline (LF, the Ctrl-j byte), intercepted at the key-event
    layer.** Trap: xterm collapses Enter and Shift-Enter to CR before `onData`; the data
    layer cannot distinguish them. Only the bare chord matches; keyup/keypress pass;
    IME composition (isComposing or keyCode 229) is left strictly alone.
    Fix: src/components/terminal/useTerminalLifecycle.ts:493-517, `src/lib/termkeys.ts:193-264`.
    Pinned: `src/lib/termkeys.test.ts` "softNewline" and "softNewlineAction" suites
    (IME, keyup, modifier, owner, latch cases).

2. **One `attachCustomKeyEventHandler` closure owns both the soft-newline chord and the
    clipboard chords.** Trap: xterm allows exactly one custom key handler; a second
    registration replaces the first. Fix: src/components/terminal/useTerminalLifecycle.ts:475-493. Pinned: structural, unpinned.

3. **Clipboard chords classify by PHYSICAL key (`code`, `keyCode` fallback), never
    `ev.key`.** Trap: on a non-Latin layout the V key types another character; a
    key-based match misses and xterm leaks `\x16` to the REMOTE agent (the original
    remote-clipboard bug). Fix: `src/lib/termkeys.ts:270-326`, src/components/terminal/useTerminalLifecycle.ts:519-534.
    Pinned: `src/lib/termkeys.test.ts` "classifies by physical key, not ev.key", "falls
    back to keyCode when code is empty".

4. **On a Mac a lone Control chord passes through to the app; Cmd-anything is the
    browser's.** Trap: vim visual-block, readline verbatim-insert and SIGINT all need
    Control to reach the app; Cmd already drives the mac clipboard.
    Fix: `src/lib/termkeys.ts:300-311`. Pinned: `src/lib/termkeys.test.ts` mac describe
    block (four tests).

5. **Ctrl-c without Shift stays SIGINT; Ctrl-Shift-c and Ctrl-Insert copy; Ctrl-v and
    Ctrl-Shift-v paste; Shift-Insert passes through.** Fix: `src/lib/termkeys.ts:316-325`.
    Pinned: `src/lib/termkeys.test.ts` classifyClipboardKey suite.

6. **A paste chord returns false WITHOUT preventDefault so the browser's native paste
    event fires and xterm's own handler reads `clipboardData` (secure-context-free).**
    Fix: src/components/terminal/useTerminalLifecycle.ts:544-559. Pinned: `components/TerminalPane.clipboard.test.tsx` "is left
    entirely to xterm: nothing uploaded, nothing cancelled".

7. **A NON-owner's paste chord takes the same path, deliberately.** Trap: swallowing the
    chord for a viewer meant no native paste event fired, so an image paste was silently
    inert instead of refused out loud (Linux/Windows only, since Cmd+v never reached the
    branch). Text still cannot leak: xterm's paste ends in `onData`, which returns early
    for a non-owner, and the server drops it too. Fix: src/components/terminal/useTerminalLifecycle.ts:549-559.
    Pinned: `components/TerminalPane.clipboard.test.tsx` "lets a viewer's paste chord
    reach the browser, so the refusal is reachable at all" and "still keeps a viewer's
    TEXT paste off the wire".

8. **The force-text-paste chord is detected independently of the classifier, before it.**
    Trap: on a Mac `Cmd+Shift+v` classifies as passthrough (the whole Cmd branch is the
    browser's), so folding the hatch into the classifier gives it to Linux only.
    Fix: src/components/terminal/useTerminalLifecycle.ts:528-533, `src/lib/termkeys.ts:328-355`.
    Pinned: `src/lib/termkeys.test.ts` "forcesTextPaste" suite ("matches Cmd-Shift-v,
    which the classifier never sees"); `components/TerminalPane.clipboard.test.tsx`
    "works on a Mac, where the chord carries Cmd and not Ctrl".

9. **The force-text latch expires on the task queue, not only on consumption.** Trap: a
    chord that produces no paste event at all (empty clipboard, OS refusal) leaves the
    latch armed and quietly disarms image handling for whatever pastes next. The native
    paste dispatches as the keydown's default action, before the task queue, so the
    timeout always lands after it. Fix: src/components/terminal/uploadPipeline.ts:454-457.
    Pinned: `components/TerminalPane.clipboard.test.tsx` "does not leave the hatch armed
    when the chord produces no paste at all" and "is one keystroke only: a plain Ctrl+v
    after it still takes the image".

10. **Copy-on-select runs inside the `mouseup` user gesture so the clipboard write is
    permitted over plain HTTP (synchronous execCommand fallback).** Fix: src/components/terminal/useTerminalLifecycle.ts:590-612;
    `src/lib/termClipboard.ts:41-54`, `src/lib/clipboard.ts`.
    Pinned: `src/lib/termClipboard.test.ts` copy suite; `src/lib/termkeys.test.ts`
    copyOnSelectAction suite.

11. **Copy/paste notifications carry NO toast id.** Trap (measured): sonner re-runs the
    close timer on every re-raise of an id; repeat copies on one id pinned "Copied to
    clipboard" open for 90 seconds across 30 copies. Each copy is its own event on its
    own clock. Fix: `src/lib/termClipboard.ts:1-19`.
    Pinned: `src/lib/termClipboard.test.ts` "raises every copy as its own notification,
    sharing no id and so no clock" (and the failure/paste variants).

12. **The mouse drag misclick floor is 2 chars; a long press copies 1.** Trap: a stray
    one-char mouse selection must not clobber the clipboard, but a finger held 400ms is
    deliberate by construction and single tokens (`y`, a digit) are ordinary terminal
    targets; refusing them copied nothing silently. Fix: `src/lib/termkeys.ts:388-419`,
    src/components/terminal/useTerminalLifecycle.ts:795-811. Pinned: `src/lib/termkeys.test.ts` "ignores a trivial one-char
    selection from a MOUSE drag" and "copies a one-char selection from a long press";
    `components/TerminalPane.test.tsx` "copies a ONE-character word, because a long
    press is deliberate".

13. **The mouse-capture hint fires at most once per PAGE SESSION (module-scope latch),
    only after a real drag, and only when the app holds the mouse.** Trap: a
    per-component ref resets on every pane remount (every agent switch); a plain click
    must not hint. Fix: src/components/terminal/pageSessionHints.ts:18-19 + src/components/terminal/constants.ts:8-12, src/components/terminal/useTerminalLifecycle.ts:591-611 + src/components/terminal/pageSessionHints.ts:39-41.
    Pinned: `src/lib/termkeys.test.ts` "hints only once per session", "does not hint on
    a plain click"; `components/TerminalPane.test.tsx` "never shows the mouse-capture
    hint, whatever the long press lands on".

14. **Right-click pastes; there is no context menu; xterm's selection-stuffing is wiped
    on `contextmenu`, touch NOT exempt, with focus handed back on the touch path.**
    Trap: xterm's right-click handler stuffs the selection into its hidden textarea
    (native-Copy prep) and left there it leaks back into the PTY as a paste; Android
    fires `contextmenu` on a long press and xterm's listener runs first (it is on a
    descendant), so the wipe must cover touch now that a long press produces a
    selection, and the focus grab would raise the soft keyboard over it.
    Fix: src/components/terminal/useTerminalLifecycle.ts:630-644, TP:774-791, src/components/terminal/inputSurface.ts:164-164.
    Pinned: `components/TerminalPane.test.tsx` "wipes the selection xterm stuffed into
    its hidden textarea on a touch long press", "hands focus back when xterm's
    contextmenu handler grabs the textarea", "leaves the textarea focused for a MOUSE
    right-click, which pastes".

15. **Right-click paste needs the async Clipboard API and is guarded against its
    SYNCHRONOUS throw.** Trap: `navigator.clipboard` is undefined over plain HTTP and
    `readText` is missing in Firefox web content; the throw is synchronous, so a bare
    promise `catch` cannot catch it; the hint points at Ctrl+v (the secure-context-free
    path). Fix: `src/lib/termClipboard.ts:56-84`.
    Pinned: `src/lib/termClipboard.test.ts` paste suite.

16. **Sticky Ctrl/Alt latches live in state plus a ref mirror, written together;
    multi-char chunks (paste/IME) pass untransformed but still clear the latch.**
    Trap: the once-created `onData` closure would capture stale state; applying a
    modifier to a paste corrupts it. Fix: src/components/terminal/inputSurface.ts:129-138, src/components/terminal/useTerminalLifecycle.ts:450-457,
    `src/lib/termkeys.ts:139-165`. Pinned: `src/lib/termkeys.test.ts` applyModifiers
    suite ("passes multi-char chunks through untransformed under every modifier").

17. **Accessory sequences honor the terminal's cursor-key mode; a latched Alt prefixes
    ESC; Ctrl on a non-char key is consumed.** Fix: src/components/terminal/inputSurface.ts:287-306,
    `src/lib/termkeys.ts:99-136` (arrowSeq, pageKeySeq).
    Pinned: `src/lib/termkeys.test.ts` arrowSeq/pageKeySeq suites.

18. **An accessory key tap never CHANGES the soft-keyboard state: refocus only when the
    typing surface had focus at tap time.** Trap: an unconditional
    `focusTypingSurface()` summoned the keyboard on every key tap while the user paged
    through output with it closed. Fix: src/components/terminal/inputSurface.ts:64-76 (`typingSurfaceHasFocus`), read
    before acting in src/components/terminal/inputSurface.ts:287-331. Pinned: `components/TerminalPane.test.tsx`
    "TerminalPane accessory keys preserve the keyboard state" suite (five tests);
    `components/AccessoryBar.test.tsx` (pointerdown preventDefault contract).

19. **Every direct input write shares one landing-effects helper: snap to the live edge,
    drop stale selection, then send.** Trap: three entry points (physical Shift-Enter,
    accessory ⇧↵, compose Send) drift apart without a shared writer; latch handling is
    deliberately left per-caller. Fix: src/components/terminal/constants.ts:41-60 (`writeInputWithLandingEffects`, `writeSoftNewline`).
    Pinned: each entry point's suite, and NEW at the rebuild
    `src/components/terminal/inputSurface.test.ts` "replays a typed key's landing
    effects once, with the first write".

20. **The accessory ⇧↵ key is the touch Shift-Enter: owner-gated, consumes latches,
    keeps focus, shares `writeSoftNewline`.** Fix: src/components/terminal/inputSurface.ts:309-321.
    Pinned: `components/TerminalPane.test.tsx` accessory suite; ComposeBar tests cover
    the in-buffer Enter (never submits).

## E. Mouse forwarding

1. **A forwarded touch gesture is a replayed DOM event, never a report dux encodes.**
    Trap (measured): protocol and encoding are separate DEC modes; hand-built SGR was
    wrong for any app without `?1006`, and the parallel cell arithmetic
    (`container.clientWidth / cols`) disagreed with xterm's on 15 of 21 probed points
    (up to two columns by the far side, the scrollbar gutter). xterm resolves the cell
    (`getMouseReportCoords`) and encodes what was negotiated.
    Fix: `src/lib/termmouse.ts` (module doc, 1-111), src/components/terminal/useTerminalLifecycle.ts:702-736,826-870.
    Pinned: `src/lib/termmouse.test.ts` "a replayed tap through xterm's pipeline"
    (SGR, X10, SGR_PIXELS, boundary cells, padding subtraction);
    `components/TerminalPane.test.tsx` tap-forwarding tests (1483-1554).

2. **The pane subscribes to `onBinary` as well as `onData`.** Trap (measured, xterm
    6.0.0): DEFAULT (X10) encoded reports go out through `triggerBinaryEvent` only; an
    `onData`-only pane drops every report from a `?1000`-without-`?1006` app, desktop
    clicks included, making it unclickable. Fix: src/components/terminal/useTerminalLifecycle.ts:459-473.
    Pinned: `components/TerminalPane.test.tsx` "sends the DEFAULT (X10 byte) encoding on
    onBinary when the app never asked for SGR" (tap and drag variants);
    `src/lib/termmouse.test.ts` "encodes DEFAULT (X10 bytes) on the BINARY channel".

3. **`onBinary` payloads are encoded latin1, never through `TextEncoder`.** Trap: X10
    puts `col + 32` in one byte; past column 95 UTF-8 splits it into two bytes and
    corrupts the report. Fix: `src/lib/termmouse.ts:231-246`, src/components/terminal/useTerminalLifecycle.ts:470-473.
    Pinned: `src/lib/termmouse.test.ts` latin1Bytes suite ("keeps a high byte as ONE
    byte, where TextEncoder would emit two").

4. **A touch drag forwards at most ONE wheel notch per touch-move, while draining the
    full row accumulator.** Trap: a fast flick's dense burst of N reports in one frame
    corrupts a mouse-tracking alt-screen pager's repaint, and the duplicated lines
    persist (no client scrollback, nothing reconnects). One notch reproduces the desktop
    wheel's 1:1-per-tick cadence. Local scrolling keeps the full magnitude. Unchanged by
    the take-over arc, and named in its blast radius only because a forwarded wheel
    report is an OWNER-only act (`forwardWheelNow` reads the verdict), so it is one of the
    behaviors the handshake seeding decides: a phantom owner would have forwarded wheel
    reports the server then dropped.
    Fix: src/components/terminal/useTerminalLifecycle.ts, `src/lib/viewport.ts:45-70` (`dragWheelReport`).
    Pinned: `src/lib/viewport.test.ts` "CAPS a fast flick to a single notch while
    draining the whole accumulator"; `components/TerminalPane.test.tsx` "sends exactly
    one SGR wheel report per move, at the finger's cell".

5. **Replayed wheel events use `deltaY` of ±1 with `DOM_DELTA_LINE`, one event per
    notch.** Trap: xterm reads only the sign, and the pixel branch accumulates a
    fractional remainder across events and swallows some. Fix:
    `src/lib/termmouse.ts:146-172`. Pinned: `src/lib/termmouse.test.ts`
    wheelReplaySteps suite ("plans one event per notch, never a bigger delta").

6. **A replayed tap's release is dispatched at the DOCUMENT; under X10 it lands on
    nothing, correctly.** Trap: xterm moves `mouseup` onto the document for the duration
    of a press (`bindMouse`); an element-only release is never seen; X10 arms no
    listener because it reports presses only. Fix: `src/lib/termmouse.ts:115-144`.
    Pinned: `src/lib/termmouse.test.ts` "plans a left press at the element then a
    release at the document", "reports a press only under the X10 protocol";
    `components/TerminalPane.test.tsx` "sends NO release under the X10 protocol".

7. **Every dux-dispatched event is tagged via a WeakSet; `isTrusted` is NOT the
    discriminator.** Trap: jsdom dispatches are never trusted, so an isTrusted guard
    makes every component test of the intercept exercise nothing; an
    assistive-technology click is untrusted AND a real intent. Fix:
    `src/lib/termreplay.ts`, `src/lib/termmouse.ts:194-228`.
    Pinned: "dux-replay tagging" suites in `src/lib/termmouse.test.ts` and
    `src/lib/termlink.test.ts`; `components/TerminalPaneLinks.test.tsx` "lets dux's own
    tagged link probe through".

8. **Local wheel speed is `scrollSensitivity: 3` and affects LOCAL scrollback only.**
    Verified against installed xterm 6 source: the option feeds the viewport's local
    scrolling, which is disabled entirely while an app captures the wheel; the
    wheel-report path stays 1:1 per event. Fix: src/components/terminal/constants.ts:14-24, src/components/terminal/useTerminalLifecycle.ts:270-270.
    Pinned: `components/TerminalPane.test.tsx` "constructs the terminal with
    scrollSensitivity 3".

9. **The scroll target is decided fresh each touch-move: normal buffer scrolls xterm
    scrollback locally; alt screen forwards wheel only when tracking is on AND we own
    the PTY; otherwise the drag does nothing.** Trap: an agent can flip in or out of an
    alt-screen TUI mid-drag; the alt screen has no xterm scrollback at all. Fix:
    src/components/terminal/useTerminalLifecycle.ts:677-679,702-736 + src/components/terminal/touchGesture.ts:114-130. Pinned: `components/TerminalPane.test.tsx` "forwards nothing on the
    alt screen when the app has no mouse tracking" and the drag-forward suite.

10. **Accessory PgUp/PgDn on the alt screen forward a screenful of wheel notches (or the
    real PgUp/PgDn keys for a keyboard-only app) at the element's center;
    top/bottom stay scrollback-only.** Trap: there is no finger to take a point from, so
    `rectCenter` stands in; jump-to-edge has no wheel equivalent. Fix: src/components/terminal/inputSurface.ts:337-414,
    `src/lib/termmouse.ts:174-185`. Pinned: `src/lib/termmouse.test.ts` rectCenter test;
    `src/lib/termkeys.test.ts` pageKeySeq suite; blur contract via
    `components/AccessoryBar.test.tsx` "every key row honors the same contract".

11. **No coordinate clamping in the replay; xterm clamps into the canvas and rejects
    out-of-grid cells itself.** A tap in the padding resolves to the edge cell exactly
    as a desktop click does. Fix: `src/lib/termmouse.ts:108-110`.
    Pinned: `src/lib/termmouse.test.ts` "clamps a point outside the canvas onto the
    edge cell".

## F. Hyperlinks

1. **dux is the sole opener of a terminal hyperlink; the click that dispatched a link is
    withheld from the app entirely. Deliberate divergence from iTerm2/Ghostty/kitty.**
    Trap: a forwarded click makes the agent CLI shell out to `open <url>` on the
    SERVER's machine, invisible to the clicker; the page opened twice, once in the wrong
    place. Fix: src/components/terminal/linkPress.ts:108-148 (rationale), `src/lib/termkeys.ts:577-637`.
    Pinned: `components/TerminalPaneLinks.test.tsx` "opens exactly one tab and reports
    nothing to the app".

2. **The suppress decision is made at PRESS time, keyed on the link dispatch alone.**
    Trap (measured, xterm 6): `bindMouse` emits the press report from element
    `mousedown` and registers the document release/drag reporters INSIDE that handler,
    so swallowing the press suppresses the whole pair; deciding at release leaks a lone
    press, which press-activated TUI controls act on; keying on the link is what keeps
    non-link TUI buttons working. Fix: src/components/terminal/linkPress.ts:124-148, src/components/terminal/linkPress.ts:179-236,
    `src/lib/termkeys.ts:602-637` (`linkPressAction`).
    Pinned: `components/TerminalPaneLinks.test.tsx` "POSITIVE CONTROL: an ordinary click
    off the link is still reported"; `src/lib/termkeys.test.ts` linkPressAction suite.

3. **Capture phase on the container, `stopPropagation` and never
    `stopImmediatePropagation`.** Trap: xterm's listeners are on descendants so capture
    decides first; stopImmediate would also silence dux's own bubble-phase
    copy-on-select. The swallowed press also does xterm's `preventDefault` + focus
    itself (xterm's mousedown never ran). Fix: src/components/terminal/linkPress.ts:132-138, src/components/terminal/linkPress.ts:216-230.
    Pinned: exercised by the whole links suite; the phase choice itself unpinned.

4. **`linkActivateAction` refuses non-primary buttons and any `detail > 1`.** Trap:
    xterm's Linkifier activates on every bare mouseup; the second click of a
    double-click (select-a-word) opened a second tab, a triple-click a third, and a
    right-click opened a tab on top of dux's paste. Scheme-gated to http(s) as defense
    in depth. Fix: src/components/terminal/useTerminalLifecycle.ts:284-295 + src/components/terminal/linkPress.ts:286-293, `src/lib/termkeys.ts:499-545`.
    Pinned: `components/TerminalPaneLinks.test.tsx` "opens only one tab for a
    double-click...", "...triple-click...", "does not open on a right-click...",
    "does not open on a middle-click"; `src/lib/termkeys.test.ts` linkActivateAction suite.

5. **The hatch chord (Cmd on a Mac, Ctrl elsewhere) forwards the click AND refuses dux's
    open; with tracking off it keeps its browser meaning.** Caveat, stated in code: Ctrl
    travels to the app as the +16 modifier bit, so a Linux visitor's hatch click arrives
    as a ctrl-click. Fix: `src/lib/termkeys.ts:461-476,535`, src/components/terminal/linkPress.ts:232-236 + src/components/terminal/pageSessionHints.ts:55-57 (one-time hint,
    only when an open actually happens).
    Pinned: `components/TerminalPaneLinks.test.tsx` "forwards a hatch-chord click to the
    app and opens nothing", "names the hatch chord on the first suppressed click";
    `src/lib/termkeys.test.ts` "refuses the hatch chord while the app is tracking the
    mouse", "reads the hatch chord per platform", "keeps chord-click opening when the
    app is not tracking the mouse".

6. **The force-local-selection modifier (Shift; Option on a Mac) is left entirely alone:
    the press passes through, xterm forwards nothing under it, and dux opens nothing.**
    Trap in both directions: treating the other platform's modifier as force-selection
    leaves a press xterm WOULD forward unsuppressed (server-side double open returns);
    swallowing it makes a link the one place the documented selection hatch fails, on
    exactly the text people select most; and since the press passes through, the
    Linkifier still activates on the drag-end mouseup, so the ACTIVATE side must refuse
    it too or a selection opens a tab. Mirrors xterm's measured
    `shouldForceSelection`. Fix: `src/lib/termkeys.ts:479-497,536-542,617-623`.
    Pinned: `components/TerminalPaneLinks.test.tsx` "selects the link locally under the
    force-selection modifier"; `src/lib/termkeys.test.ts` "refuses the force-selection
    modifier while the app is tracking the mouse", "reads the force-selection modifier
    per platform", "keeps a force-selection-modifier click opening when nothing is
    tracking", "leaves a press alone under the force-selection modifier".

7. **The link under the press is resolved SYNCHRONOUSLY by priming xterm's own
    Linkifier, never by trusting passive hover.** Trap: the buffer scrolls under a still
    pointer, the first click of a page may follow no mousemove, and a resize clears the
    current link; each leaks a server-side open or a stale-true that swallows a TUI
    button press. The whole provider chain is synchronous in installed xterm 6.
    Fix: src/components/terminal/linkPress.ts:73-80, src/components/terminal/linkPress.ts:192-197, `src/lib/termlink.ts:113-171` (`primeLinkHover`).
    Pinned: `components/TerminalPaneLinks.test.tsx` "suppresses a press on a link that
    no mousemove ever hovered", "does not swallow an unhovered click off the link after
    hovering it"; `src/lib/termlink.test.ts` primeLinkHover suite.

8. **The prime starts with a mouseleave and a far-side priming move at a DIFFERENT
    cell.** Trap (measured in the container): the Linkifier re-runs providers only when
    the pointer's CELL changes; a second tap on the same link reported the same cell,
    the hover was skipped, and the tap opened 0 tabs. A point outside the element
    resolves to no cell and would leave the stale cell in place. Accepted nit: the
    far-side prime can flicker an underline on another link sharing the row.
    Fix: `src/lib/termlink.ts:142-171`. Pinned: `src/lib/termlink.test.ts` "primes from
    a different point before hovering the tapped one".

9. **Every link-probe event is `bubbles: false` at `.xterm-screen`.** Trap: xterm's
    mouse-report listener is one level up on `Terminal.element`; a bubbling or
    element-targeted move is encoded and sent, and a 1003 any-motion app receives two
    fabricated motion reports per click. Fix: `src/lib/termlink.ts:27-38,98-111,127-140`.
    Pinned: `src/lib/termlink.test.ts` "does not bubble, so xterm's focus grab,
    selection and mouse reports never see it" and "does not bubble, so a 1003 any-motion
    app sees no motion reports".

10. **A swallowed press is always paired with its release; non-primary buttons never
    touch the in-flight record; the outside-release watcher observes and clears, never
    swallows.** Traps: a chorded right press wiping the record leaves the left release
    to leak alone (a release for a gesture the app never saw begin); a swallowing
    one-shot would eat an unrelated mouseup after an off-window release (alt-tab with
    the button down); a new primary press always clears the previous gesture so a lost
    release cannot wedge the next click. Fix: src/components/terminal/linkPress.ts:150-188, src/components/terminal/linkPress.ts:239-251.
    Pinned: `components/TerminalPaneLinks.test.tsx` "keeps a swallowed press paired when
    a right press chords into it", "never swallows an unrelated mouseup after a release
    off-window", "emits no report for a press that slides off the link, and does not
    wedge".

11. **A travelled gesture opens only if it stayed on the SAME link; within the drag
    threshold it is a click.** Fix: src/components/terminal/linkPress.ts:253-267, `src/lib/termkeys.ts:639-659`
    (`linkReleaseOpens`). Pinned: `src/lib/termkeys.test.ts` linkReleaseOpens suite;
    `components/TerminalPaneLinks.test.tsx` "emits no report for a press that slides off
    the link...".

12. **Swallowed-but-not-opened is a real state.** Trap: forwarding a press dux will not
    act on hands the app a press with no release; it happens for multi-click tails and
    for links dux would refuse anyway (preference off under an on-screen link, bad
    scheme). Fix: `src/lib/termkeys.ts:624-637`.
    Pinned: `components/TerminalPaneLinks.test.tsx` "stays quiet when a suppressed click
    opens nothing", "leaks neither a second tab nor a report on a double-click";
    `src/lib/termkeys.test.ts` "swallows the tail of a multi-click gesture without
    opening", "swallows without opening when the preference or the scheme refuses".

13. **All opens go through ONE function: same truth table, same
    `noopener,noreferrer`, same activation counter the touch probe reads.**
    Fix: src/components/terminal/linkPress.ts:88-104 (`openTerminalLink`; Linkifier activate and the capture release are
    its only two callers). Pinned: the links suite counts `window.open` calls across
    both paths; `src/lib/termlink.test.ts` activation-counter tests.

14. **OSC 8 is gated at the parser when hyperlinks are disabled (consume, render plain);
    when enabled it falls through to xterm's handler and the linkHandler gates to
    http(s).** Links created before a live toggle persist until rewritten (stated).
    `registerAgentNotifications` deliberately does not register OSC 8. Fix:
    src/components/terminal/useTerminalLifecycle.ts:314-323, src/components/terminal/liveValues.ts:61-63 + TP:271. Pinned: `components/TerminalPaneLinks.test.tsx` "does not
    open when the hyperlinks preference is off"; `src/lib/agentNotifications.test.ts`
    "does not register OSC 8 (the pane owns the hyperlink gate)".

15. **A touch tap probes the Linkifier by replaying the suppressed synthetic mouse
    sequence; a tap that opened a link forwards nothing and does NOT focus compose.**
    Trap: preventDefault on touchend suppressed the only events that can activate a
    link, so a tapped link used to just raise the keyboard; and a link tap must not pull
    the caret into the compose box (the user is leaving; matches the desktop). Runs
    inside the touchend user gesture so the `window.open` is not a popup.
    Fix: src/components/terminal/useTerminalLifecycle.ts:826-870, `src/lib/termlink.ts:48-111,173-215`.
    Pinned: `src/lib/termlink.test.ts` activateLinkAtPoint and terminalTapAction suites;
    `components/TerminalPane.test.tsx` "forwards a tap to a mouse-tracking app AND
    focuses compose" (ordinary-tap control).

## G. Touch selection

1. **dux drives xterm's OWN selection model through public `Terminal.select`; the
    browser cannot select terminal text at all.** Trap: the browser synthesizes mouse
    events for a tap and nothing else, so xterm's selection service never sees a touch
    drag; `xterm.css` sets `user-select: none` on `.xterm` (deliberate, left alone).
    Fix: src/components/terminal/selectionDrag.ts:1-33, `src/lib/termselect.ts:1-33`.
    Pinned: `src/lib/termselect.xterm.test.ts` (pure helpers against a real xterm
    buffer).

2. **`select()` is forward start-plus-length and the length WRAPS by column count
    (measured, xterm 6.0.0 `SelectionModel.finalSelectionEnd`).** Any anchor-to-focus
    span is `(endRow - startRow) * cols + (endCol - startCol)`, ends ordered first.
    Fix: `src/lib/termselect.ts:18-25,351-378` (`selectionSpan`).
    Pinned: `src/lib/termselect.xterm.test.ts` "wraps a multi-row span through the
    length, which is what makes this work at all"; `src/lib/termselect.test.ts`
    selectionSpan suite.

3. **Long-press (400ms, still) vs scroll (8px moved) disambiguation; a short still tap
    trips neither and reaches xterm as a focus tap; any second finger cancels.**
    Trap: lifting one finger out of a pinch used to take the selecting branch and copy;
    the whole gesture is cancelled, not just the timer, and the painted selection stays
    (the user may be pinching to read it). Fix: src/components/terminal/touchGesture.ts:62-65, src/components/terminal/touchGesture.ts:86-111.
    Pinned: `components/TerminalPane.test.tsx` "selects the word under the finger on a
    long press", "leaves no selection behind when the gesture was a scroll", "a second
    finger cancels the pending long press", "a second finger during an ACTIVE selection
    cancels the gesture".

4. **The word rules are xterm's own: the default `wordSeparator` set character for
    character (measured), blank runs expand, a non-blank separator selects itself.**
    Reason: a long press and a desktop double-click are the same intent and must pick
    the same word. Fix: `src/lib/termselect.ts:137-241`.
    Pinned: `src/lib/termselect.test.ts` wordRangeAt suite;
    `src/lib/termselect.xterm.test.ts` word tests.

5. **A word is followed across wrapped lines, joined at the seam (both seam cells
    non-separators); blank runs never chase a wrap.** The archetypal target is a long
    file path, exactly the thing that wraps. Fix: `src/lib/termselect.ts:243-308`
    (`wordSpanAt`), src/components/terminal/selectionDrag.ts:107-110 (`isWrapped` via public API).
    Pinned: `src/lib/termselect.test.ts` "wordSpanAt across wrapped lines" suite;
    `src/lib/termselect.xterm.test.ts` "follows a wrapped path across the physical line
    break", "stops at the hard break...";
    `components/TerminalPane.test.tsx` "carries the selection onto the next row".

6. **Every column crosses `glyphAt` before arithmetic, in BOTH drag directions; the
    focus width applies only on a forward drag.** Trap: on the right half of a wide
    glyph the raw column is the width-0 continuation cell; a backwards drag that skipped
    the resolve started the span mid-glyph, dropping the glyph and prepending a stray
    blank. Measured widths under xterm's default Unicode v6 provider: emoji are ONE
    cell, CJK and fullwidth forms are two. Fix: `src/lib/termselect.ts:91-180,331-378`,
    src/components/terminal/selectionDrag.ts:159-167. Pinned: `src/lib/termselect.test.ts` glyphAt suite, "takes a WIDE
    glyph whole at the start of a backwards drag", "does not let the focus width leak
    into a backwards drag"; `src/lib/termselect.xterm.test.ts` CJK/fullwidth/emoji
    tests; `components/TerminalPane.test.tsx` "starts a backwards drag at the wide
    glyph, not inside it".

7. **Cell math measures `.xterm-screen`, never the pane container, and clamps into the
    grid.** Trap: the container is wider by the scrollbar gutter (374px vs 361px
    measured on a 390px phone); dividing it by the column count drifts two columns by
    the far side. A zero-sized rect means not laid out, no cell to answer.
    Fix: src/components/terminal/selectionDrag.ts:88-97, `src/lib/termselect.ts:55-89` (`pointToCell`).
    Pinned: `src/lib/termselect.test.ts` pointToCell suite ("agrees with a whole-row
    sweep computed from the true cell width").

8. **Edge auto-scroll is a 50ms TIMER walking one row per tick, not per-event and never
    a magnitude.** Trap: a finger parked past the edge produces no further events, so
    an event-driven version stopped dead; xterm's own drag scroll is a 50ms interval for
    the same reason; a magnitude would rocket through the scrollback at touchmove rates.
    Fix: src/components/terminal/selectionDrag.ts:52-192 (`SELECT_SCROLL_INTERVAL_MS`, `autoScrollTick`),
    `src/lib/termselect.ts:380-393` (`edgeAutoScroll`).
    Pinned: `components/TerminalPane.test.tsx` "keeps auto-scrolling while the finger is
    parked past the bottom edge", "auto-scrolls the other way above the top edge",
    "stops auto-scrolling when the finger comes back inside", "stops auto-scrolling the
    moment the finger lifts"; `src/lib/termselect.test.ts` edgeAutoScroll suite.

9. **Every row crosses `viewportY` into ABSOLUTE buffer space in exactly one place;
    the auto-scroll re-selects from the stored finger point.** Fix: src/components/terminal/selectionDrag.ts:100-104,
    src/components/terminal/selectionDrag.ts:143-167. Pinned: `components/TerminalPane.test.tsx` "reads the word from the
    SCROLLED-BACK row, not from the top of the buffer", "follows the viewport as the
    auto-scroll moves it under the finger".

10. **A buffer flip mid-gesture abandons the selection; the painted highlight stays.**
    Trap: a normal-buffer row number applied to the alt buffer names unrelated content;
    abandoning is the only honest answer. Fix: src/components/terminal/selectionDrag.ts:78-82, src/components/terminal/selectionDrag.ts:147-153.
    Pinned: `components/TerminalPane.test.tsx` "abandons the gesture when the app flips
    buffers mid-drag".

11. **The selection lift is CANCELLED (`preventDefault` on touchend).** Trap: the
    browser's compatibility mouse events after an uncancelled touchend do three wrong
    things: xterm's mousedown focuses its textarea (keyboard over the selected text),
    `_handleSingleClick` wipes the highlight the copy was for, and over a
    mouse-tracking app the click is forwarded, breaking "selects locally, forwards
    nothing". A drag is incidentally protected by touchmove's preventDefault; the bare
    press-and-lift is the primary gesture and is not, and Chrome's own long-press
    threshold sits above dux's. Fix: src/components/terminal/useTerminalLifecycle.ts:770-792.
    Pinned: `components/TerminalPane.test.tsx` "cancels the lift so no compatibility
    mouse events follow it".

12. **Copy happens on the LIFT, inside the touchend user gesture, through the same
    `copyOnSelectAction` and preference as the mouse; no refocus afterwards.**
    Trap: the execCommand fallback needs the gesture on plain-HTTP origins; pulling
    focus back would throw the soft keyboard over the selection; the hint answer is
    meaningless here (the long press already selected locally with no modifier).
    Fix: src/components/terminal/useTerminalLifecycle.ts:788-811. Pinned: `components/TerminalPane.test.tsx` "copies on lift and
    leaves the selection painted", "copies nothing on lift when copy-on-select is off",
    "does not raise the soft keyboard".

13. **A long press ALWAYS selects locally, even under mouse tracking: it is the touch
    equivalent of the desktop force-local-selection modifier.** Claude Code and
    opencode both take the mouse; forwarding would leave every real agent pane
    unselectable by finger. Fix: src/components/terminal/selectionDrag.ts:21-26.
    Pinned: `components/TerminalPane.test.tsx` "selects locally over a mouse-tracking
    app and forwards nothing".

14. **The next tap clears the selection, before the redirect's own early returns.**
    Trap: dismiss-on-tap must work with the compose bar off and for a non-owner too.
    Fix: src/components/terminal/useTerminalLifecycle.ts:813-818. Pinned: `components/TerminalPane.test.tsx` "clears the selection
    on the NEXT tap".

15. **A long press on blank space is still not a tap.** The gesture is a selection from
    the timer's fire whatever it landed on; the lift must not focus or raise the
    keyboard. Fix: src/components/terminal/touchGesture.ts:108-111. Pinned: `components/TerminalPane.test.tsx` "selects
    nothing where there is nothing, rather than the nearest word".

16. **iOS callout and Android long-press context menu are suppressed over the
    terminal.** `-webkit-touch-callout: none` on the container (Safari's loupe/share
    menu over the gesture); `contextmenu` preventDefault on the touch pointer type only
    (a mouse right-click still pastes). Fix: TP:760-792.
    Pinned: `components/TerminalPane.test.tsx` "suppresses the platform callout and
    context menu on the terminal".

17. **The selection buzz is double-guarded.** Safari has no Vibration API; a browser
    that does may throw without user activation; a missing buzz never fails a
    selection. Fix: src/components/terminal/selectionDrag.ts:134-140. Pinned: unpinned.

18. **A programmatic select leaves xterm's mouse selection working afterwards.**
    Pinned as a library fact: `src/lib/termselect.xterm.test.ts` "a programmatic
    selection and the mouse afterwards" (three tests). No pane code; listed so the
    rewrite does not add compensation for a problem that does not exist.

## H. Compose bar, typing surfaces, focus

1. **Width decides the LAYOUT; the pointer decides the TYPING SURFACE. Two orthogonal
    rules.** Trap: keying the bar to width swapped the typing surface under the user on
    tablet rotation; `pointer: coarse` does not change with orientation. A landscape
    tablet gets the desktop layout AND the buffered input; the bars travel with the
    pointer into the desktop shell. Fix: TP:168-196, `src/lib/composebar.ts:107-170`.
    Pinned: `components/TerminalPane.test.tsx` "TerminalPane typing surfaces follow the
    pointer, not the layout" and "compose bar gate" ("does NOT change when only the
    viewport width changes"); `src/lib/composebar.test.ts` mode suites.

2. **The setting wins; only `auto` consults the transient per-device toggle.** Trap
    (measured): an Android tablet with and without a keyboard case reports identical
    interaction media queries, so only the user can resolve it; the toggle writes
    localStorage, never config. Fix: `src/lib/composebar.ts:172-217`,
    TP:974-978. Pinned: `src/lib/composebar.test.ts` "lets always and never win over
    the device-local choice"; `components/TerminalPane.test.tsx` "TerminalPane
    typing-surface toggle" suite ("writes the choice to localStorage, so a reload does
    not snap back", "does not write the ui.compose_bar setting", "is absent when the
    setting has already decided").

3. **`never` keeps the accessory KEYS on a coarse pointer; `always` brings the pair to a
    fine one.** The preference is about the compose BOX; a soft keyboard still cannot
    produce a Ctrl chord. Fix: `src/lib/composebar.ts:150-170` (`touchSurfacesApply`).
    Pinned: `src/lib/composebar.test.ts` "keeps the accessory keys on a phone whose
    compose box is switched off".

4. **The input-menu surface switch is offered one state wider than the in-bar toggle.**
    Trap: `auto` on a FINE pointer with a stored `compose` choice mounts the message box
    but not the accessory bar; the only way back lived in the bar that was not there.
    Accepted asymmetry: from a fine pointer, switching to Direct removes every anchor
    (state is pixel-identical to the default). Fix: `src/lib/composebar.ts:219-246`.
    Pinned: `src/lib/composebar.test.ts` "is offered on a fine pointer once a choice is
    stored"; `components/TerminalPane.test.tsx` "offers the surface switch on a fine
    pointer with a stored choice".

5. **The compose DRAFT lives in the pane's state, not in ComposeBar.** Trap: a
    preference flip or rotation past the breakpoint unmounts the bar; the draft must
    survive it, and losing/regaining ownership too. Fix: TP:339, TP:1005-1014.
    Pinned: `components/TerminalPane.test.tsx` "keeps in-progress text across a
    compose-bar unmount (pref flip off and on)".

6. **Send is macro-convention keystrokes plus a SEPARATE, delayed bare CR; an empty
    Send is one immediate bare CR.** Trap (measured): Claude Code 2.1.217 merges stdin
    chunks into one paste through a 50ms debounce and force-classifies >800-char key
    events as paste; a CR within the window is swallowed into the paste as a newline.
    150ms is 3x with margin. Deliberately NOT bracketed paste: the wire must keep "line
    break" (Alt+Enter, ESC CR) and "Enter" distinct. Whitespace-only is text.
    Fix: `src/lib/composebar.ts:34-70`, src/components/terminal/inputSurface.ts:83-282.
    Pinned: `src/lib/composebar.test.ts` composeSendWrites suite incl. "pins the submit
    delay comfortably above Claude Code's 50ms paste debounce";
    `components/TerminalPane.test.tsx` "Send writes the body first and the submitting CR
    as a DELAYED second write", "a multiline body uses Alt+Enter newlines", "an empty
    Send is ONE immediate bare CR".

7. **Every refused Send keeps the buffer and toasts why, on the fixed `compose-send`
    id; three refusals: not owner, socket not open, over the client cap.** Trap: a
    composed message is minutes of typing, not a re-typable keystroke; the readyState
    guard drops silently; an oversized frame makes the server abort the whole socket
    (16 MiB cap), so the 2 MiB client cap fails one send instead. The fixed id is
    deliberate (the reason replaces itself; three presses want one toast), with the
    countdown-restart hazard accepted and bounded. Fix: src/components/terminal/inputSurface.ts:231-263,
    `src/lib/composebar.ts:16-32`; server `crates/dux-web/src/server.rs:207`.
    Pinned: `components/TerminalPane.test.tsx` "a Send while the socket is down keeps
    the buffer and toasts", "an oversized Send keeps the buffer and toasts";
    `src/lib/composebar.test.ts` composeSendTooLarge suite;
    `components/ComposeBar.test.tsx` "keeps the buffer when onSend reports failure".

8. **The delayed CR is skipped if the pane unmounted or the socket dropped before the
    timer fired.** Trap: an orphaned CR delivered to a socket the pane no longer drives.
    Fix: src/components/terminal/inputSurface.ts:268-282. Pinned: `components/TerminalPane.test.tsx` "skips the delayed CR
    when the pane unmounted before it fired", "...when the socket dropped in between".

9. **Send does not consume the one-shot Ctrl/Alt latches.** A latch arms the next direct
    KEY; a composed message is not a key. Fix: src/components/terminal/inputSurface.ts:226-229.
    Pinned: NEW at the rebuild, `src/components/terminal/inputSurface.test.ts`
    "are NOT consumed by Send: a latch arms the next KEY, and a message is not a key".

10. **The tap-to-focus redirect: with the bar up and this client the owner, a plain tap
    preventDefaults the touchend and focuses the compose textarea; over a
    mouse-tracking app the swallowed click is restored via the mouse replay, with focus
    put back.** Trap: xterm grabs focus from the SYNTHETIC mousedown after touchend;
    full-screen TUI menus are driven by exactly the click the redirect swallows.
    Preference off / desktop / non-owner: taps reach xterm untouched. The listener is
    registered non-passive unconditionally (touchend passivity gates nothing).
    Fix: src/components/terminal/useTerminalLifecycle.ts:737-870. Pinned: `components/TerminalPane.test.tsx` "TerminalPane
    tap-to-focus redirect" suite ("a tap preventDefaults and focuses the compose
    textarea", "does not intercept the tap when the preference is off", "forwards a tap
    to a mouse-tracking app AND focuses compose").

11. **The compose-insert sink registers exactly while the bar renders (mobile, pref on,
    owner) and retires with an only-retire-your-own guard.** Trap: a picked macro must
    become an editable DRAFT, never an immediate PTY write, but the mobile picker lives
    in MobileShell's header, outside the pane; a successor pane may already have
    replaced the registration. Fix: TP:589-613, `src/lib/composeInsert.ts`.
    Pinned: `components/TerminalPane.test.tsx` "TerminalPane compose macro insert sink"
    suite (register/retire/takeover/unmount, "insert lands the text in the draft at the
    caret and writes NOTHING to the PTY").

12. **The pane registers its typing surface for the header's macro picker
    (`terminalFocus`), resolved at CALL time.** Trap: Base UI's default
    return-to-trigger means the review Enter re-presses the popover trigger and reopens
    the menu; the surface must be where typing is NOW, not where it was at registration.
    Fix: TP:610-618, `src/lib/terminalFocus.ts`. Pinned:
    `components/TerminalPane.test.tsx` "registers its typing surface so the header's
    picker can return focus to it".

13. **`focusTypingSurface` is the one routing rule (compose textarea while the bar is
    up, xterm's hidden textarea otherwise); every refocus goes through it.**
    Fix: src/components/terminal/inputSurface.ts:50-55. Pinned: every focus test,
    and NEW at the rebuild `src/components/terminal/inputSurface.test.ts` "the one
    focus-routing rule" suite (all four tests). The rule is now a standalone
    exported function both the hook and the pane call, so there is one
    implementation by construction.

14. **A draft splice records its intended caret and an effect applies it in the commit
    the new value reaches the DOM; the updater is StrictMode-idempotent.** Trap: a
    controlled textarea re-render parks the caret at the end; the selection is read
    once, up front; null selection appends; out-of-range clamps; reversed reorders.
    Fix: src/components/terminal/inputSurface.ts:147-156, src/components/terminal/inputSurface.ts:169-196, `src/lib/composebar.ts:87-105`.
    Pinned: `src/lib/composebar.test.ts` insertIntoComposeDraft suite;
    `components/TerminalPane.test.tsx` "insert lands the text in the draft at the
    caret...", "insert moves focus to the compose textarea".

15. **Regaining ownership focuses the freshly mounted compose box AFTER the commit.**
    Trap: `takeOver`'s own focus call runs before the bar mounts (ref still null) and
    falls back to xterm. Fix: TP:579-588. Pinned: exercised by the sink
    retire/restore test; unpinned in isolation.

16. **The compose textarea has its own paste listener (element-registered, no capture).**
    Trap: the bar renders OUTSIDE the terminal container (a sibling row), so the
    container's capture listener cannot see a paste landing in it, and on a phone that
    is where pastes land. Fix: TP:620-645.
    Pinned: `components/TerminalPane.clipboard.test.tsx` "pasting an image while the
    mobile compose bar is the typing surface" suite.

17. **The unfocused caret is a solid block while the compose bar is up, the conventional
    outline otherwise, applied live.** Trap: with the bar up xterm is never focused by
    design, so the hollow outline states something false all session. Verified: the
    option is mutable in place on xterm 6.0.0. Fix: TP:410-421, src/components/terminal/useTerminalLifecycle.ts:262-266,
    `src/lib/composebar.ts:248-268`. Pinned: `components/TerminalPane.test.tsx`
    "TerminalPane inactive cursor style" suite (open solid, open outline, follow the
    toggle on the SAME terminal); `src/lib/composebar.test.ts` inactiveCursorStyle.

18. **A scroll gesture blurs BOTH possible keyboard holders (xterm's textarea and the
    compose textarea); input keys keep focus, only page-scroll keys blur; blur is
    touch-gated.** Trap: on iOS the textarea stays the focused element after the user
    swipes the keyboard down, so any focus-retaining button tap pops it back up; a
    narrow-window mouse user must not silently lose terminal focus when paging.
    Fix: src/components/terminal/useTerminalLifecycle.ts:693-700, src/components/terminal/inputSurface.ts:349-414. Pinned: `components/AccessoryBar.test.tsx` "every
    key row honors the same contract (arrows and page scroll included)";
    `components/TerminalPane.test.tsx` keyboard-state suite.

19. **The input ⋯ menu: items computed before the trigger exists, exactly one instance
    on screen in every bar state, and its own third-anchor row when neither bar is up.**
    Trap: an ⋯ opening an empty popup is reachable (fine pointer, stored compose
    choice, uploads off); the state "keys up, box off, top bar hidden" used to render
    two menus; a chrome-free PWA screen has no browser Back button, so the menu's own
    row is the way back. A fine-pointer desktop grows no new row. Fix: TP:455-485,
    TP:952-953, TP:1037-1048, `src/lib/inputMenu.ts`.
    Pinned: `components/TerminalPane.test.tsx` "TerminalPane input menu anchors" suite
    ("renders exactly one menu with the keys up, the box off and the top bar hidden",
    "renders its own row when neither bar is up"), "input menu follows the touch
    surfaces" suite, "offers nothing on a fine pointer with an empty item list".

20. **A non-owner's menu carries the top-bar toggle only.** Attach and the surface
    switch are input; the keys toggle would be a write with no visible effect on the
    viewer's screen that re-hides the OWNER's keys. Fix: TP:457-471.
    Pinned: `components/TerminalPane.test.tsx` "TerminalPane input menu for a non-owner"
    suite.

21. **The accessory bar is additionally gated on `ui.mobile_accessory_bar` with
    optimistic override.** Fix: TP:199-201, TP:977-985.
    Pinned: `components/TerminalPane.test.tsx` "TerminalPane mobile accessory-bar
    preference" suite.

22. **xterm's hidden textarea gets all four input-mangling attributes set explicitly.**
    Trap: xterm documents some, but the defaults are unreliable across versions and
    mobile browsers (autocorrect in particular still fires), and a shell has no buffer
    for them to fix. The compose textarea is the opposite: native autocorrect ON,
    that is its whole point. Fix: src/components/terminal/useTerminalLifecycle.ts:388-396.
    Pinned: compose half by `components/ComposeBar.test.tsx` "enables native
    autocorrect, autocapitalize, and spellcheck on the textarea"; the xterm half
    unpinned.

23. **The compose placeholder follows the pane's KIND: agent panes ask for a message,
    every terminal (session, project, standalone) asks for a command.** Fix:
    TP:1024-1031, ComposeBar constants. Pinned: `components/TerminalPane.test.tsx`
    "TerminalPane compose placeholder follows the surface" suite.

24. **Selection-focus on mount is owner-only; a read-only observer gets no focus grab.**
    Fix: src/components/terminal/useTerminalLifecycle.ts:562-574. Pinned: covered by the non-owner suites; unpinned in isolation.

25. **`composeActiveRef` lags the rendered value by one commit and both mismatch
    directions degrade gracefully.** Stale false falls to `term.focus()`; stale true at
    worst redirects one tap into a just-unmounted bar (no-op on the null ref).
    Fix: src/components/terminal/liveValues.ts:80-88. Pinned: unpinned (stated tolerance).

26. **The macro trigger no longer floats over the PTY text; both entry points live in
    headers outside the pane.** Fix: TP:813-820. Pinned:
    `components/TerminalPane.test.tsx` "renders no macro trigger over the terminal on
    desktop" / "on mobile".

## I. Viewer suppression and notifications

1. **The browser xterm is a VIEWER; dux-core's alacritty_terminal is the authoritative
    emulator, so the viewer must not answer device/status/color queries.** Trap: xterm's
    auto-replies go through `onData`, the keystroke path, into the shared PTY a second
    time; the duplicate arrives a beat later and is typed at an idle prompt as literal
    garbage (`]10;rgb:...`, `[?1;2c`). Installed before `open()` so it is armed before
    any byte. Suppressed: DA1/DA2, DSR 5/6 and the DEC-private form, DECRQM (both),
    DECRQSS, OSC 4/10/11/12 query forms. Fix: src/components/terminal/useTerminalLifecycle.ts:298-302,
    `src/lib/suppressViewerReports.ts:1-80`.
    Pinned: `src/lib/suppressViewerReports.test.ts` incl. the vanilla-xterm positive
    control ("vanilla xterm answers device/status queries via onData") and "suppresses
    every device/status/color query reply once installed".

2. **Only the QUERY form of a color OSC is swallowed; a SET falls through so the viewer
    still recolors.** Mirrors xterm's own "?"-slot split exactly.
    Fix: `src/lib/suppressViewerReports.ts:31-39,76-79`.
    Pinned: `src/lib/suppressViewerReports.test.ts` "matches a query slot, not a color
    value", "lets an OSC 11 color SET through so the viewer still recolors".

3. **NEW at 39f5c2ce: focus reports raised while a REPLAY chunk is parsing are dropped;
    the window is a counter bounded by the write's own completion callback, and on the
    reconnect drain path only the first held chunk (the replay itself) gets the
    window.** Trap (measured, xterm 6.0.0): DECSET 1004 in the mode-restore tail makes
    `CoreBrowserTerminal` immediately volunteer `ESC [ I`/`ESC [ O` through `onData`;
    every replay applied to an unfocused pane typed a spurious focus-OUT at the child,
    and the claude CLI reacts to focus state internally. A counter, not a flag, so
    overlapping writes cannot close the window early; never a timer; real transitions
    outside the window still reach the PTY. Fix: src/components/terminal/useTerminalLifecycle.ts:445-449 (the
    drop in `onData`), src/components/terminal/attachReplay.ts:107-123 (`writeReplayChunk`, `replayWritesInFlight`), src/components/terminal/attachReplay.ts:167-175 (drain path,
    first chunk only); `src/lib/suppressViewerReports.ts:82-109` (`isFocusReport`, the
    measured mechanism).
    Pinned: `components/TerminalPane.test.tsx` "TerminalPane focus reports raised by a
    replay": "drops a focus report the replay chunk provokes", "still forwards a
    genuine focus report once the replay has landed", "suppresses the focus report on
    the RECONNECT drain path too", "closes the window on the write CALLBACK, not on a
    timer"; mechanism by `src/lib/suppressViewerReports.test.ts` "a mode restore that
    turns focus reporting on" (both tests).

4. **Agent OSC notification sequences (9, 99, 777) bridge to browser Notifications,
    gated on enabled + permission granted + backgrounded, throttled leading-edge.**
    Registered beside suppressViewerReports so both viewer hooks are armed before the
    first byte; OSC 9 progress (`4;<digits>`) never notifies. Fix: src/components/terminal/useTerminalLifecycle.ts:303-313,
    `src/lib/agentNotifications.ts`.
    Pinned: `src/lib/agentNotifications.test.ts` (classification, gating, throttle,
    registration/dispose suites).

5. **OSC 52 clipboard passthrough honors the resolved `clipboard_passthrough` mode
    (focused/always/off); off consumes without writing; a read query is consumed and
    never answered; notifications and clipboard are gated independently.**
    Fix: src/components/terminal/liveValues.ts:64-66 + TP:272, `src/lib/agentNotifications.ts`.
    Pinned: `src/lib/agentNotifications.test.ts` OSC 52 tests ("writes the clipboard
    under focused/always, never under off", "read query is consumed without a clipboard
    write", "notifications fire with the clipboard sealed...", keep-last throttle).

6. **The notification title matches the OWNER exhaustively, never the nullable id
    pair.** Trap: collapsing an unrecognized owner into two nulls named a terminal
    "Agent", wrong and unfixable downstream; the in-arm fallback covers a lookup miss,
    a different condition. Fix: TP:217-233, TP:87-99.
    Pinned: `components/TerminalPane.test.tsx` "titles desktop notifications with the
    project name, not 'Agent'".

7. **Repeat notifications from one target replace instead of stack (stable per-target
    tag).** Fix: src/components/terminal/useTerminalLifecycle.ts:310-313. Pinned: unpinned directly (tag string asserted nowhere).

8. **Preference toggles for notifications, hyperlinks, clipboard mode, copy-on-select
    and attention grace are all read lazily through refs so no toggle recreates the
    terminal.** Fix: src/components/terminal/liveValues.ts:45-46 + TP:262, TP:268-421. Pinned: structural; exercised by the live
    settings tests; unpinned as a rule.

## J. Drop, upload, paste-to-file

1. **A drop saves the file and pastes its PATH; bytes never go to the terminal.**
    Settled premise: no agent CLI reads a file from its input stream; every measured
    emulator inserts the path. Fix: `src/lib/fileDrop.ts:1-13`, upload loop
    src/components/terminal/uploadPipeline.ts:195-348. Pinned: `components/TerminalPane.filedrop.test.tsx` "uploads it and
    the bare path reaches the socket with nothing that submits", "sends an awkward path
    byte for byte as it is on disk".

2. **The drag surface exists only when the feature is ON, and not-yet-known is NOT
    enabled.** Trap: bootstrap loads in parallel with the workspace; defaulting the
    unknown window to on advertised a drop the server would refuse. A viewer and a
    phone get no drag handling at all (no overlay, no preventDefault).
    Fix: TP:147-158, src/components/terminal/uploadPipeline.ts:523-544 (`paneAcceptsFileDrag`, deliberately not named
    `dragCarriesFiles`), `src/lib/fileDrop.ts:19-37`.
    Pinned: `components/TerminalPane.filedrop.test.tsx` "offers nothing at all when
    file drop is switched off", "offers nothing while the setting is not known yet",
    "never appears for a viewer who does not hold input", "offers both the overlay and
    the upload when file drop is on".

3. **A feature flip mid-drag retires the overlay during RENDER (adjust-state-on-input
    pattern), in both flip directions; the depth counter is pinned to zero while no
    drag is active.** Traps: with the gate closed no dragleave/drop will ever clear the
    overlay; an effect would paint the stale overlay once; off-then-on must not revive
    a drag that ended while off; leftover depth would demand that many extra
    dragleaves. Fix: TP:366-393. Pinned: `components/TerminalPane.filedrop.test.tsx`
    "takes the overlay back if the setting arrives disabled mid-drag".

4. **`dragDepth` counts enter/leave pairs.** Trap: dragging across a child fires a
    parent dragleave; a boolean flickers the overlay off over every internal boundary.
    Fix: TP:134-138, TP:696-722. Pinned: overlay tests (J2, J3) exercise it;
    unpinned in isolation.

5. **Uploads are SEQUENTIAL, in dropped order, which is also path-send order; one
    spinner per file, one report per drop, on an id minted per drop.** Trap: two quick
    drops sharing an id lose the first drop's report under the second's spinner; the
    report is often the refusal list. Fix: src/components/terminal/uploadPipeline.ts:195-374 (`handleUploadedFiles`,
    `runUpload`), `src/lib/fileDrop.ts:894-914` (`nextFileDropToastId`).
    Pinned: `components/TerminalPane.filedrop.test.tsx` "finishes each upload and sends
    its path before the next one starts", "puts the spinner and the report on ONE id,
    so the final replaces it", "counts through a multi-file drop", "still ends in a
    final toast when something throws unexpectedly".

6. **The sink (terminal paste vs compose draft) is resolved at the GESTURE; its
    availability is re-asked immediately before EACH delivery; a vanished compose box
    strands the file rather than falling back to the terminal sink.** Traps: ownership
    moves and sockets close between two files; a silent socket write would be reported
    as sent with nothing written; a mid-batch destination change makes the toast's
    wording wrong for every file on one side of the switch. The terminal sink pastes
    through xterm's own `paste()` (bracketed when negotiated), which the compose path
    deliberately refuses (a path contains no newline, so the compose reason does not
    apply). Fix: src/components/terminal/uploadPipeline.ts:44-192 (`UploadSink`, both sinks, `activeUploadSink`).
    Pinned: `components/TerminalPane.filedrop.test.tsx` "says it was saved but not
    sent, and gives its full path", "does not claim a paste when the socket has
    closed", "brackets the path when the running program asked for bracketed paste";
    `components/TerminalPane.clipboard.test.tsx` "reports the file as stranded when the
    box goes away mid-upload", "puts the path in the DRAFT and sends nothing".

7. **The paste FORM and the char LIMIT resolve together, per file, from refs: what the
    focused tab's LIVE process launched with wins over what config says for its
    provider.** Traps: a closure-snapshotted profile outlives a config reload or
    provider retarget mid-drop; if current config won, per-tab publishing would buy
    nothing and two tabs launched either side of an edit could not differ.
    Fix: src/components/terminal/liveValues.ts:73-74 + TP:275, src/components/terminal/liveValues.ts:69-77 + TP:274-276, src/components/terminal/uploadPipeline.ts:290-316; `src/lib/fileDrop.ts:196-406`.
    Pinned: `components/TerminalPane.filedrop.test.tsx` "gives each pane the form its
    OWN tab launched with", "keeps the running form after a config edit, then takes the
    new one on relaunch", "drops the dead process's form when the tab goes dormant";
    `src/lib/fileDrop.test.ts` dragDropPasteFormFor suite.

8. **A TERMINAL always gets `single_quoted`, reading no provider config at all.** Trap:
    the first version sent a terminal's path bare "because a terminal runs a shell, not
    that CLI", exactly backwards; dux permits `$`, backticks, spaces, semicolons and
    quotes in paths, and a shell EVALUATES them, so a shell needs more protection, not
    less. POSIX-only, stated; a non-POSIX form is added only once measured.
    Fix: `src/lib/fileDrop.ts:260-286` (`TERMINAL_PASTE_FORM`), src/components/terminal/uploadPipeline.ts:206-212.
    Pinned: `components/TerminalPane.filedrop.test.tsx` "gives a TERMINAL the shell-safe
    path, whatever its owning agent runs", "makes a hostile path inert in a TERMINAL",
    "keeps a terminal path with an apostrophe one word"; `src/lib/fileDrop.test.ts`
    "gives a TERMINAL the shell-safe form, and reads no provider at all".

9. **The quoting forms are measured per-CLI and proven by lexing: single-quote
    close-escape-reopen for apostrophes; double-quote escapes all four significant
    characters losslessly; backslash form escapes ASCII shell-significants only.**
    Trap: an earlier double-quote version escaped only two of four on a wrong premise
    (lexing removes the backslash, so the full escape costs nothing and protects a
    future evaluating reader); over-escaping CJK paths makes prompts unreadable.
    One file per paste (a newline SUBMITS; two paths in one paste become plain text);
    one trailing space, no newline. Fix: `src/lib/fileDrop.ts:446-544`.
    Pinned: `src/lib/fileDrop.test.ts` pastePayload suites, each backed by the
    `posixLex` property checks ("lexes to exactly ONE token, which is the path, for
    every input").

10. **The attachment char limit is keyed by the COMMAND'S FILE NAME (codex: 1000),
    never by form and never by provider block name, measured against the PAYLOAD in
    code points.** Traps: codex files any longer paste away as generic content before
    recognizing a path (the toast would claim success while nothing attached); keying
    by form gave a terminal codex's limit and let a re-formed codex escape it; keying
    by name failed both directions (`[providers.myagent] command = "codex"` is a real
    codex); the quoting itself adds characters; `.length` double-counts emoji.
    Deliberately NOT a config setting: it is a measurement of a third-party CLI.
    Fix: `src/lib/fileDrop.ts:334-444`, src/components/terminal/uploadPipeline.ts:305-316.
    Pinned: `components/TerminalPane.filedrop.test.tsx` "holds a long path back from
    codex on EVERY form it can be configured with", "holds a long path back from a real
    codex running under another name", "sends a long path to a different CLI that
    merely happens to be named codex", "sends a very long path to a TERMINAL", "still
    sends a path that only just fits"; `src/lib/fileDrop.test.ts` attachment-limit
    suite ("measures the FINAL payload, not the path on disk", "counts characters, not
    UTF-16 code units", "puts the boundary exactly where the CLI puts it").

11. **One toast per drop, chosen from an ordered four-rung ladder (nothing saved /
    saved-not-sent / refused / success); a worse outcome can never be reported as a
    better one; renames and folder scatter are stated on every rung that has a saved
    file.** Fix: `src/lib/fileDrop.ts:692-892` (`dropToastFor`), src/components/terminal/uploadPipeline.ts:329-348.
    Pinned: `src/lib/fileDrop.test.ts` dropToastFor suites ("lets the worse outcome win
    over successes, at every rung", "keeps every rename, whichever rung the toast lands
    on", rename/breakdown/reason-grouping/punctuation tests).

12. **Sticky is decided per rung: the stranded-path rung (the full path exists nowhere
    else on screen) and a text paste that saved NOTHING (the paste was cancelled to
    make room for the file, so the text survives only on the clipboard). A failed FILE
    drop is not sticky; a partial refusal is not.** Fix: `src/lib/fileDrop.ts:186-198,
    708-731,843-855`. Pinned: `src/lib/fileDrop.test.ts` "which drop reports wait for
    the user" suite (all six tests).

13. **The toast claims "sent", never "arrived" or "pasted".** Trap: nothing acknowledges
    a PTY socket write; a take-over between the courtesy check and the frame reaching
    the server makes the server drop it silently. Fix: src/components/terminal/uploadPipeline.ts:322-326,
    `src/lib/fileDrop.ts:59-72`. Pinned: `src/lib/fileDrop.test.ts` "claims only that
    the path was SENT, never that it was pasted".

14. **The folder label travels with each FILE, not the drop.** Trap: a terminal's
    directory changes on `cd` mid-batch; one label reported the last folder for all.
    Fix: src/components/terminal/uploadPipeline.ts:270-279, `src/lib/fileDrop.ts:39-54,546-598`.
    Pinned: `src/lib/fileDrop.test.ts` "never claims one folder for files that went to
    two", "groups the breakdown by folder rather than listing every file", "an agent's
    files share one folder and are never broken down".

15. **Refusal wording is status-aware; only 503 earns retry advice, and never twice.**
    Trap: the server's own 503 body already says "Try the drop again shortly."; welding
    the local tail on comma-spliced the advice into the server's sentence.
    Fix: `src/lib/fileDrop.ts:74-123`, src/components/terminal/uploadPipeline.ts:258-266.
    Pinned: `src/lib/fileDrop.test.ts` "what a refused upload is reported as" suite;
    `components/TerminalPane.filedrop.test.tsx` "says a busy server is temporary, which
    a 413 does not".

16. **Image paste reads the `paste` EVENT, never the async clipboard API, decided
    synchronously while the event is cancellable.** Trap: dux is routinely served over
    plain HTTP (Tailscale), where `navigator.clipboard.read()` is blocked; a
    kind-string `DataTransferItem` yields its contents only asynchronously, after xterm
    has already pasted; `getData("text/plain")` on the event needs no secure context.
    Fix: src/components/terminal/uploadPipeline.ts:419-480, `src/lib/clipboardPaste.ts:17-27`.
    Pinned: `components/TerminalPane.clipboard.test.tsx` image suite;
    `src/lib/clipboardPaste.test.ts` (decision matrix).

17. **The image listener is a CAPTURE-phase paste listener on the container, coexisting
    with the Ctrl+v key intercept.** Trap: the key handler cannot carry clipboard
    contents and the paste event carries no modifiers, so the two halves meet only
    through the armed latch; xterm's handler is on the hidden textarea INSIDE the
    container, so capture on the ancestor decides first; ordinary text passes through
    untouched. Fix: src/components/terminal/useTerminalLifecycle.ts:646-651, src/components/terminal/uploadPipeline.ts:429-442.
    Pinned: `components/TerminalPane.clipboard.test.tsx` "is left entirely to xterm",
    "skips image handling and hands the whole event to xterm".

18. **Image wins over text in one event; force-text beats both; non-image file items and
    image-typed STRING items are left alone; a viewer's image or long-text paste is
    refused OUT LOUD with nothing saved, on one toast id per subject.** Traps: a rich
    copy puts `image/png` beside `text/plain`; svg markup of kind string is text
    someone copied; a silent viewer refusal is a keystroke that did nothing; image and
    text refusals sharing an id erase each other. Fix:
    `src/lib/clipboardPaste.ts:221-298`, src/components/terminal/uploadPipeline.ts:488-520.
    Pinned: `src/lib/clipboardPaste.test.ts` (image-wins, force-text, non-image,
    refusals); `components/TerminalPane.clipboard.test.tsx` "keeps the image refusal on
    its own toast id", "refuses it for a client that does not hold input, and saves
    nothing".

19. **Long-text-paste-to-file is agent-only STRUCTURALLY (the terminal variant carries
    no threshold field), strictly-greater in CODE POINTS with a capped scan, off at 0
    and for older servers.** Traps: a long paste into a shell is a command or heredoc;
    bytes or UTF-16 units bias against CJK/emoji; the count runs in the paste handler
    before cancellation (spread costs 218ms/180MB on 20M chars, the scan 37ms, capped
    ordinary case 0.1ms); an unpaired surrogate counts as one on both halves (threshold
    and file agree). Fix: `src/lib/clipboardPaste.ts:40-54,300-385`, TP:159-164,
    src/components/terminal/uploadPipeline.ts:479-480.
    Pinned: `src/lib/clipboardPaste.test.ts` long-text suite ("measures the threshold
    in CHARACTERS...", "turns a LONE SURROGATE into U+FFFD...", byte-for-byte tests);
    `components/TerminalPane.clipboard.test.tsx` "fires at exactly one character over a
    threshold the server chose", "pastes long text verbatim into a TERMINAL, at any
    length", "is switched off by a threshold of 0", "is switched off for a server that
    never published the setting".

20. **dux-invented names (`pasted-<local clock>.<ext>`) exist only when the clipboard
    supplied none; a supplied name is never rewritten; extensions come from an explicit
    mime table with a safe fallback.** Trap: server validates rather than rewrites
    names, so an invented name must be one it accepts; `image/svg+xml` derived
    naively becomes `.svg+xml`. Fix: `src/lib/clipboardPaste.ts:130-215`.
    Pinned: `src/lib/clipboardPaste.test.ts` pastedImageName/pastedTextName suites;
    `components/TerminalPane.clipboard.test.tsx` "keeps a non-Latin file name all the
    way to the server".

21. **The upload request carries the TERMINAL socket's connection id, not the events
    socket's.** Trap: the server refuses a PTY id in the header the other API modules
    stamp. Fix: src/components/terminal/uploadPipeline.ts:250-253.
    Pinned: `components/TerminalPane.filedrop.test.tsx` "carries the terminal socket's
    own connection id, not the events one".

22. **The picker is the third gesture into the same journey and adds no pipeline; the
    sink resolves AFTER the picker settles; the open call must spend the activating
    click's user activation; the hidden input is unconditional inside the pane.**
    Trap: row menus attach through a pane rendering no input rows at all, so the input
    cannot live with the conditional bars; `pastedTextChars` is never passed (it would
    describe a gesture that did not happen). Fix: src/components/terminal/uploadPipeline.ts:380-400 (`attachFromPicker`),
    TP:806-812, `src/hooks/use-file-picker.tsx`.
    Pinned: `components/TerminalPane.filedrop.test.tsx` "attaching a file from the
    picker" suite (order, cancel, terminal destination).

23. **The attach capability is published to the row menus only while this pane is
    mounted, OWNS input, and uploads are on; retirement is stale-safe.** Trap: a
    viewer's attach would strand every file saved-not-sent. Fix: src/components/terminal/uploadPipeline.ts:404-417,
    `src/lib/attachRegistry.ts`.
    Pinned: `components/TerminalPane.filedrop.test.tsx` "is not offered at all when
    file drop is switched off", "is not offered by a viewer's pane", "retires the
    capability when the pane unmounts"; `src/lib/attachRegistry.test.ts` ("a stale
    retirement does not remove the live registration").

24. **There is deliberately NO `status_clear_seconds` ref in the pane; notify reads the
    window at raise time.** Trap: the paste listener closes over the MOUNT render,
    where bootstrap has usually not arrived; a captured value pinned every
    clipboard-paste toast to the pre-bootstrap default for the pane's life.
    Fix: TP:436-442 (the absence, documented), `src/lib/notify.ts`.
    Pinned: `components/TerminalPane.clipboard.test.tsx` "uses the setting that arrived
    after mount, not the one missing at mount".

25. **The drop overlay is pointer-events-none, names the destination KIND (not a path),
    and shows only to the input holder.** Trap: an overlay that can swallow the drop it
    advertises; the terminal's real folder is discovered server-side at upload time.
    Fix: TP:729-747. Pinned: `components/TerminalPane.filedrop.test.tsx` "appears
    while a file is over the pane and names where it will land", "says the terminal's
    CURRENT folder, because a shell moves".

## K. Assorted lifecycle

1. **The mount effect re-runs only on `[kind, id, sessionId, ptyUrl]`; every other input
    reaches its closures through refs.** Trap: listing a component-body function or a
    bootstrap field tears down and rebuilds the terminal (and its socket) on every
    render or refetch; the eslint-disable comments at each site carry the reason.
    Fix: src/components/terminal/useTerminalLifecycle.ts:1099-1105, ref mirrors at TP:140-421. Pinned: structural; regression shows
    up as churn in almost every suite; unpinned as a named rule.

2. **All module-scope registrations (active PTY socket, compose sink, terminal focus
    target, attach capability) retire ONLY their own registration.** Trap: on a focus
    switch React's old-cleanup / new-effect order is not guaranteed; an unconditional
    clear nulls the incoming pane's registration. Fix: src/components/terminal/useTerminalLifecycle.ts:1078-1093, TP:590-600,
    TP:610-618; `src/lib/attachRegistry.ts`.
    Pinned: `src/lib/attachRegistry.test.ts` "a stale retirement does not remove the
    live registration"; sink/socket variants exercised by the sink and unmount tests.

3. **Unmount closes the socket deliberately (no reconnect), retires the connection id,
    nulls the term/fit refs, disposes subscriptions, notifications and the OSC 8 gate,
    and clears every timer the effect armed.** Fix: src/components/terminal/useTerminalLifecycle.ts:1053-1098.
    Pinned: `src/lib/ptySocket.test.ts` "does not reconnect after a user-initiated
    close"; `components/TerminalPane.test.tsx` unmount halves of the ownership and sink
    tests.

4. **The viewed ping: every 2s while owner AND visible, immediately on gaining
    ownership, and grace-gated after a hidden-to-visible transition (one ping fires at
    the grace boundary; hiding again cancels it).** Traps: a read-only observer or a
    backgrounded owner pinging suppresses attention for everyone on the shared engine
    (a PTY socket stays open in a background tab); an instant clear on tab return
    dismissed the attention flag before the returning user saw it; window focus covers
    the never-hidden desktop case; initial load observes no transition and gets no
    grace. Server-side the ping never claims ownership. Fix: src/components/terminal/useTerminalLifecycle.ts:988-1049,
    TP:422-430, `src/lib/viewedPing.ts`;
    `crates/dux-web/src/server.rs:1626-1631`.
    Pinned: `src/lib/viewedPing.test.ts` (all three suites; the grace vectors are twins
    of `dux_core::focus::within_attention_grace`'s, kept identical by declaration).

5. **The pane serves four targets through two prop shapes: agent session-slot tab
    (`id === sessionId`, session PTY route), extra tab (nested tab route), and a
    terminal by OWNER (session/project/standalone routes, exhaustive switch).**
    Trap: the nullable `sessionId`/`projectId` pair is lossy on purpose (lookups only);
    anything that must SAY something about the owner matches exhaustively.
    Fix: TP:76-99 + src/components/terminal/useTerminalLifecycle.ts:101-158, `src/lib/ptySocket.ts:48-118` (`terminalSocketUrl` ends in
    `assertNever`). Pinned: `src/lib/ptySocket.test.ts` URL-builder suite ("routes each
    owner kind to its own socket"); `components/TerminalPane.test.tsx`
    "TerminalPane project-terminal owner resolution" suite.

6. **Session lookups go by `sessionId`, never by `id`.** Trap: for an agent, `id` is the
    FOCUSED TAB id; an extra tab's differs from the session id, so a lookup by `id`
    misses the session. Fix: TP:203-213. Pinned: exercised by the extra-tab tests;
    unpinned in isolation.

7. **The pointer type of the most recent press is a per-interaction signal, not a width
    check.** Trap: `isMobile` misclassifies a touchscreen laptop with a mouse; Android
    fires `contextmenu` on a touch long-press, which is dux's selection gesture, while
    a mouse right-click must still paste. Fix: TP:438-445, TP:760-770.
    Pinned: `components/TerminalPane.test.tsx` "leaves the textarea focused for a MOUSE
    right-click, which pastes" vs the touch contextmenu tests.

8. **Overlay precedence is fixed: connection-lost beats the spinner beats nothing; the
    take-over card yields to connection-lost; readiness/reconnect overlays are
    pointer-events-none.** Fix: TP:821-895.
    Pinned: `components/TerminalPane.test.tsx` "shows the Reconnect affordance on
    'failed' without doubling the spinner" and C12's test.

9. **`inColumn` decides the pane's flex role; the menu's own row counts as company.**
    Trap: leaving the third-anchor row out of the column test made the desktop shell
    drop the row carrying the way back. Bars take height OUT of the terminal in the
    desktop shell (panel geometry untouched); the pane's own RO reports the reflow.
    Fix: TP:475-485, TP:905-939. Pinned: `components/TerminalPane.test.tsx` input
    menu anchor suite ("renders its own row when neither bar is up").

10. **Both hint latches are MODULE scope: once per page session, surviving the pane
    remounts every agent/tab switch causes.** Fix: src/components/terminal/pageSessionHints.ts:18-19 + src/components/terminal/constants.ts:8-12.
    Pinned: `components/TerminalPaneLinks.test.tsx` "names the hatch chord on the first
    suppressed click" (fires once); `src/lib/termkeys.test.ts` "hints only once per
    session".

## Known limits, explicitly accepted

- **Scrollback-trim selection shift.** `selectAnchor` holds absolute buffer rows; when
  the ring is full and the child writes, xterm trims the top, every absolute row
  shifts, and the anchor names different content for the rest of the gesture. The
  public API publishes no trim signal (xterm compensates internally from `lines.onTrim`,
  which is private), no `length`/`baseY` combination distinguishes scrolled from
  trimmed at the cap, and every inferable proxy misfires. Assessed and deliberately not
  guarded: it needs a busy writer during a one-second drag, costs a wrong selection and
  nothing else, and lifting and pressing again fixes it. src/components/terminal/selectionDrag.ts:58-73.

- **X10 (`?9`) tracking loss on attach.** `dux_core::pty`'s mode restore has no X10
  flag to re-assert because alacritty_terminal does not model one, so a `?9` app's
  tracking is lost the moment a browser attaches (measured in the preview container).
  No measured CLI uses `?9`. Not the browser's to fix. `src/lib/termmouse.ts:91-96`.

- **Live-stream focus-report residual.** The focus-report suppression window covers
  replay chunks only. A DECSET 1004 arriving on the LIVE stream (an app enabling focus
  reporting mid-session while the pane is unfocused) still provokes one volunteered
  report outside any window, and a real focus transition during a replay is dropped
  outright (nothing re-emits it when the window closes, so the child's focus belief
  stays stale until the next transition). Both are the narrow ends of the trade stated at src/components/terminal/useTerminalLifecycle.ts:445-449 and src/components/terminal/attachReplay.ts:107-123.

- **Overlapping-reopen drain window (inherited, pre-rebuild).** If a second (re)open
  lands while the previous reconnect's drain callback is still pending, the new
  replay is queued behind the stale one and both are written after one reset,
  stacking two histories. Needs a socket flap inside a single xterm parse window;
  byte-similar at the pre-rebuild pane. Tracked, not fixed here
  (src/components/terminal/attachReplay.ts, the drain block).

- **Bracketed-paste asymmetry between sinks.** The terminal upload sink pastes through
  xterm's `paste()` (bracketed when negotiated); compose Send refuses bracketed paste
  by design. Both directions are documented at their sites (src/components/terminal/uploadPipeline.ts:144-152,
  `src/lib/composebar.ts:52-56`) and are intentional, not drift.

- **Compose-send toast countdown restart.** Repeating a failing Send restarts the error
  toast's window each time, so it lingers a full window after the LAST attempt. Correct
  end of the trade for a message still true while the user retries; bounded. src/components/terminal/inputSurface.ts:241-246.

- **Far-side link prime underline flicker.** The priming move can hover, then leave, a
  different link sharing the row; last write wins in the same tick, cost is one frame
  of underline. `src/lib/termlink.ts:137-140`.

- **Fill-font adjacency.** 383 code points render in the fill face beside symbols-face
  neighbours; a mismatched typeface is readable and tofu is not.
  `src/lib/terminalFont.ts:39-51`.

- **Cold-cache FOUT.** The terminal opens against fallback metrics for one frame on a
  cold font cache, then refits; warm cache resolves near-instantly.
  `src/lib/terminalFont.ts:145-148`.

- **Focus-while-hidden edge on the editor-style info surfaces does not apply here; the
  pane's own accepted freshness gap is the background tab:** rAF throttling means a
  resize received while hidden refits late or not at all; the foreground resync is the
  designed recovery, not a bug. src/components/terminal/resizeCoordinator.ts:320-322.
