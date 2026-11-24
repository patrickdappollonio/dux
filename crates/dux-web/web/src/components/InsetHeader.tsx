import { PanelRightOpen } from "lucide-react"
import { useMemo, useRef, useState, type RefObject } from "react"

import { AppMenu } from "@/components/AppMenu"
import { MacroPopover } from "@/components/MacroPopover"
import { readStackMemory } from "@/lib/stackMemory"
import { PaneMenu } from "@/components/PaneMenu"
import { CHIP_GLYPHS } from "@/components/headerChipGlyphs"
import {
  insetHeaderChips,
  insetHeaderPaneSubject,
} from "@/components/insetHeaderView"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { NewTabButton, StackNewDialog } from "@/components/StackNewDialog"
import { TheaterToggle } from "@/components/TheaterToggle"
import { HEADER_BUTTON_ROW, HeaderButton } from "@/components/HeaderButton"
import { FoldHoldContext, useFoldToFit } from "@/hooks/use-fold-to-fit"
import { useMacrosBlockedHint } from "@/hooks/use-macros-blocked"
import { useTheaterToggleFocusWhen } from "@/hooks/use-theater"
import {
  addToAgentBlockHint,
  useAgentChangeBlock,
  useRowDriverLabel,
} from "@/hooks/use-agent-active-elsewhere"
import { useAgentRowWatch } from "@/lib/paneCover"
import { useIsTruncated } from "@/hooks/use-truncated"
import { useIsNarrowComputerLayout } from "@/hooks/use-mobile"
import { headerChipTooltip, type HeaderChip } from "@/lib/headerSubject"
import type { SessionView } from "@/lib/types"
import { changesSummary } from "@/lib/changesSummary"
import { formatRegularCount } from "@/lib/formatRegularCount"
import {
  changesPaneEffectivelyHidden,
  changesSpacerPercent,
  showChangesPane,
  toggleTheater,
  useDux,
  type SelectedTarget,
} from "@/lib/store"
import { cn } from "@/lib/utils"

// The desktop center-pane top bar: one row of chips naming what you are looking
// at, each a glyph followed by its value, then the pane's controls on the right.
// Which chips exist and what each says lives in `lib/headerSubject.ts`, which
// subject is asked in `insetHeaderView.ts`; this module is how they are drawn.

// One glyph-and-value pair. The two shrink weights make the primary chip give
// way last: every chip is `min-w-0`, but a non-primary chip's shrink factor is
// thousands of times the primary one's, so the rest of the row yields first.
// `overflow-hidden` lets the glyph clip too, because a lone glyph with no value
// beside it says nothing.
function Chip({ chip }: { chip: HeaderChip }) {
  const Glyph = CHIP_GLYPHS[chip.kind]
  // Re-measured on the value and on the chip's own box, so the reveal is
  // correct while the split is being dragged and not only after it settles.
  const { ref, truncated } = useIsTruncated<HTMLSpanElement>(chip.value)
  return (
    <SimpleTooltip content={headerChipTooltip(chip, truncated)}>
      <span
        data-chip={chip.kind}
        className={
          "flex min-w-0 items-center gap-1.5 overflow-hidden text-sm text-muted-foreground " +
          (chip.primary ? "shrink" : "shrink-[9999]")
        }
      >
        <Glyph aria-hidden className="size-3.5 shrink-0" />
        <span
          ref={ref}
          data-fold-name
          className="min-w-0 truncate font-medium text-foreground"
        >
          {chip.value}
        </span>
      </span>
    </SimpleTooltip>
  )
}

// The header's controls that can fold into the pane's `⋯`, the first to fold
// first.
const FOLD_ORDER = ["theater", "macros", "new-tab"] as const

export function InsetHeader() {
  const dux = useDux()
  const { spine, selectedSessionId, selectedTarget } = dux
  const session = spine?.sessions.find((s) => s.id === selectedSessionId)
  const chips = insetHeaderChips(
    spine,
    session,
    selectedTarget,
    // The tab on top of the stack, read the way the stack reads it.
    session ? (dux.stackTabs[session.id] ?? readStackMemory(session.id).tab ?? undefined) : undefined,
    // Under the take-over card nothing says the agent has terminals.
    useAgentRowWatch(session?.id).watched,
  )

  const paneSubject = insetHeaderPaneSubject(spine, session, selectedTarget)
  // The agent New tab… adds to, on an agent's pane only.
  const newTabSession =
    paneSubject?.kind === "agent" ? paneSubject.session : null
  // THE NAMES OUTRANK THE BUTTONS. Whenever a chip's own text would be cut off
  // beside them, the header's controls leave one at a time for rows in the
  // pane's `⋯`: Theater first, then Macros…, then New tab…, and they come back
  // in the reverse order. Only where that `⋯` exists, since a folded control
  // must have somewhere to go.
  const paneMenuRef = useRef<HTMLButtonElement | null>(null)
  const hasNewTab = newTabSession !== null
  const foldItems = useMemo(
    () => FOLD_ORDER.filter((item) => item !== "new-tab" || hasNewTab),
    [hasNewTab],
  )
  // What the reopen control says about the pane it brings back: the count and
  // its word, as the control prints it. Null while no agent is in view, which
  // is also the state in which the phone draws no count control at all.
  const summary = changesSummary(dux.changes, session?.id)
  const reopenWords = summary ? formatRegularCount(summary.count, "file") : undefined
  const { isFolded, settled, rowRef, itemRef, hold } = useFoldToFit({
    enabled: Boolean(paneSubject && selectedTarget),
    items: foldItems,
    // The reopen control stands beside the names and is as wide as its words,
    // which change with the count ("9 files" to "10 files"), so a change in
    // them is re-measured as a change in the names is.
    watch: [...chips.map((c) => c.value), reopenWords ?? ""].join("\u0000"),
    focusOnFold: paneMenuRef,
  })
  const folded = {
    newTab: newTabSession !== null && isFolded("new-tab"),
    theater: isFolded("theater"),
    macros: isFolded("macros"),
  }

  // The width the header must hold back on its right so that whatever sits just
  // before it lands on the terminal pane's RIGHT EDGE rather than the window's.
  // A percentage, mirrored from the panel group below (see the store), because
  // the header is that group's sibling and spans the same width: no pixel is
  // measured, nothing has to know the pane's size, and it stays correct at any
  // zoom. Zero while the Changes pane is hidden, so the button slides right with
  // the terminal pane that just grew under it.
  const narrow = useIsNarrowComputerLayout()
  const spacer = changesSpacerPercent(dux, narrow)
  const paneHidden = changesPaneEffectivelyHidden(dux, narrow)

  // The same sentence names the control and fills its tooltip. It holds the
  // visible words, so the count is said once and the name contains the label.
  const reopenName = reopenWords
    ? `Show Changes pane, ${reopenWords}`
    : "Show Changes pane"

  return (
    // Whatever a control opens, and the pane's own menu, holds the fold while
    // it is open, so nothing leaves from under it.
    <FoldHoldContext.Provider value={hold}>
      <header className="relative flex h-12 shrink-0 items-center gap-2 border-b px-3">
        {/* The upward continuation of the changes-panel divider. Absolutely
            positioned so the offset resolves against the header's full box: a
            border on the control cluster resolves against the header's padded
            interior instead, and lands a few pixels left of the line it claims to
            continue. Being outside the flex flow, it also collects no `gap-2` and
            cannot push the pane's controls off the pane edge.

            The calc's pixel term compensates for the panel handle: the group lays
            out as terminal, a 1px handle, changes, so a pure percentage of the
            full width lands spacer/100 px left of the real divider, which is a
            visible one-pixel step under browser zoom. */}
        {/* Not on a narrow window, where the cluster is wider than the pane and
            the line would cross the app menu's button. */}
        {!paneHidden && !narrow && (
          <span
            aria-hidden="true"
            data-testid="changes-divider-continuation"
            className="pointer-events-none absolute inset-y-0 w-px bg-border"
            style={{ right: `calc(${spacer}% - ${spacer / 100}px)` }}
          />
        )}
        {/* The chips share one shrink budget so the header clips instead of
            pushing the right-hand controls off the edge. Once a chip's text is
            cut off the pane's controls fold away to give them room, and past
            that the chips yield all the way to nothing.

            The gap is wider than the header's own `gap-2` because it replaces the
            hairline dividers a field list would otherwise need; the glyphs
            already say where one field stops. One font at one size throughout,
            because the fields are peers. */}
        <div
          ref={rowRef}
          className="flex min-w-0 flex-1 items-center gap-3.5 overflow-hidden"
        >
          {chips.map((chip) => (
            <Chip key={chip.kind} chip={chip} />
          ))}
        </div>

        {/* The pane's own controls, one outline family at the `h-8` token, 40px
            under a finger, each a header child of its own so a folded one leaves
            no gap behind. New tab… leads on an agent's pane, then Theater, then
            the macro quick-picker, labelled because macros are a feature people
            forget exists. */}
        {selectedTarget && newTabSession && !folded.newTab ? (
          <span ref={itemRef("new-tab")} className="flex shrink-0">
            <NewTabButton session={newTabSession} />
          </span>
        ) : null}
        {/* Only with the `⋯` its row folds into: a target is selected a moment
            before its agent reaches the list, and Theater must not stand alone. */}
        {selectedTarget && paneSubject && !folded.theater ? (
          <span ref={itemRef("theater")} className="flex shrink-0">
            <TheaterToggle settled={settled} />
          </span>
        ) : null}
        {selectedTarget && paneSubject && !folded.macros ? (
          <span ref={itemRef("macros")} className="flex shrink-0">
            <MacroPopover target={selectedTarget} />
          </span>
        ) : null}
        {/* The way back to a hidden or zero-width Changes pane, whose own menu is
            gone with it, so it never folds into this `⋯`. It says how many files
            the pane holds, in words, since nothing else here says what the agent
            changed. */}
        {paneHidden && (
          <SimpleTooltip content={reopenName}>
            <HeaderButton
              icon={<PanelRightOpen />}
              label={reopenWords}
              aria-label={reopenName}
              onClick={() => showChangesPane()}
            />
          </SimpleTooltip>
        )}
        {paneSubject && selectedTarget ? (
          <HeaderPaneMenu
            subject={paneSubject}
            target={selectedTarget}
            newTabSession={newTabSession}
            folded={folded}
            triggerRef={paneMenuRef}
          />
        ) : null}

        {/* The spacer IS the cog's cluster, not an empty box in front of it:
            sized to the Changes panel's percentage and right-aligned, it puts the
            cog on the window's right edge and the pane's `⋯` on the terminal
            pane's, out of one number and no measurement. `min-w-fit` keeps the
            cog whole at 0% and in a pane dragged narrower than it. While the
            reopen control shows, the pane is laid out at nothing, so the spacer is
            the cog alone and the reopen control ahead of the `⋯` costs no edge. */}
        <div
          data-testid="changes-pane-spacer"
          className={cn(HEADER_BUTTON_ROW, "min-w-fit justify-end")}
          style={{ width: `${spacer}%` }}
        >
          <AppMenu />
        </div>
      </header>
    </FoldHoldContext.Provider>
  )
}

// The pane's top menu on a computer, the twin of the phone flap's `⋯`. It opens
// the whole menu, the same body the sidebar row's `⋯` opens, rather than a
// header-sized subset, and it sits with the pane's own controls rather than in
// the cog beside it, whose menu is the app's. Which body it opens is the
// question the header's own chips answer.
//
// While the header has folded its controls in, it carries them as rows, and
// what they open has no button left to stand on: the New tab… dialog and the
// macro picker open here once the menu has finished closing, the picker placed
// against this `⋯`. Focus comes back to the `⋯` from the dialog, and from the
// picker goes to the pane's typing surface when it has one.
function HeaderPaneMenu({
  subject,
  target,
  newTabSession,
  folded,
  triggerRef,
}: {
  subject: NonNullable<ReturnType<typeof insetHeaderPaneSubject>>
  target: SelectedTarget
  newTabSession: SessionView | null
  /// Which of the header's controls are folded into this `⋯` now.
  folded: { newTab: boolean; theater: boolean; macros: boolean }
  triggerRef: RefObject<HTMLButtonElement | null>
}) {
  const { theater } = useDux()
  const newTabBlock = useAgentChangeBlock(newTabSession)
  const newTabDevice = useRowDriverLabel(
    newTabSession === null ? null : { sessionId: newTabSession.id },
  )
  const newTabBlocked =
    newTabBlock === null ? null : addToAgentBlockHint(newTabBlock, newTabDevice)
  const pendingRef = useRef<"new-tab" | "macros" | null>(null)
  const [adding, setAdding] = useState(false)
  const [macrosOpen, setMacrosOpen] = useState(false)
  const macrosBlocked = useMacrosBlockedHint(target)
  // An exit from theater that its own button did not press gives the focus to
  // this `⋯` when the button that would take it is folded in here.
  useTheaterToggleFocusWhen(triggerRef, !theater && folded.theater)
  return (
    <>
      <PaneMenu
        subject={subject}
        // The pane it is painted over, which is what the INPUT group is read
        // under: a companion terminal's pane publishes under the TERMINAL's id
        // while the menu around it is the agent's.
        pane={target}
        appearance="header"
        // No drill: the cog is mounted in this same header for as long as this
        // `⋯` exists, so drilling would offer one body twice.
        settingsDrill={false}
        triggerRef={triggerRef}
        folded={
          folded.newTab || folded.theater || folded.macros
            ? {
                newTab:
                  newTabSession && folded.newTab
                    ? () => (pendingRef.current = "new-tab")
                    : undefined,
                newTabBlocked,
                theater: folded.theater ? () => toggleTheater() : undefined,
                macros: folded.macros
                  ? () => (pendingRef.current = "macros")
                  : undefined,
                macrosBlocked,
              }
            : undefined
        }
        onCloseComplete={() => {
          const pending = pendingRef.current
          pendingRef.current = null
          if (pending === "new-tab") setAdding(true)
          if (pending === "macros") setMacrosOpen(true)
        }}
      />
      {newTabSession ? (
        <StackNewDialog
          session={newTabSession}
          open={adding}
          onClose={() => setAdding(false)}
        />
      ) : null}
      <MacroPopover
        target={target}
        anchored={{ ref: triggerRef, open: macrosOpen, onOpenChange: setMacrosOpen }}
      />
    </>
  )
}
