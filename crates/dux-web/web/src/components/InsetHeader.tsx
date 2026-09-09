import { PanelRightOpen } from "lucide-react"

import { AppMenu } from "@/components/AppMenu"
import { MacroPopover } from "@/components/MacroPopover"
import { PaneMenu, type PaneMenuSubject } from "@/components/PaneMenu"
import { CHIP_GLYPHS } from "@/components/headerChipGlyphs"
import { insetHeaderChips } from "@/components/insetHeaderView"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { TheaterToggle } from "@/components/TheaterToggle"
import { Button } from "@/components/ui/button"
import { useIsTruncated } from "@/hooks/use-truncated"
import { headerChipTooltip, type HeaderChip } from "@/lib/headerSubject"
import { changesSummary } from "@/lib/changesSummary"
import {
  changesPaneEffectivelyHidden,
  changesSpacerPercent,
  showChangesPane,
  useDux,
} from "@/lib/store"
import { matchOwner } from "@/lib/terminalOwner"

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
        <span ref={ref} className="min-w-0 truncate font-medium text-foreground">
          {chip.value}
        </span>
      </span>
    </SimpleTooltip>
  )
}

export function InsetHeader() {
  const dux = useDux()
  const { spine, selectedSessionId, selectedTarget } = dux
  const focusedTerminal =
    selectedTarget?.kind === "terminal" ? selectedTarget : undefined
  const session = spine?.sessions.find((s) => s.id === selectedSessionId)
  const chips = insetHeaderChips(spine, session, selectedTarget)

  // What the pane menu is about, decided the same way the chips are: the agent
  // when one is behind the pane, the terminal itself when nothing is. A
  // session-owned terminal takes the agent's menu, and its own Close and editor
  // entries ride along as a labelled group because the menu is handed the pane
  // as well as the subject.
  //
  // Read off the target, not `selectedSessionId`, which can still name the
  // agent a project terminal was reached from, and matched exhaustively on the
  // owner so a fourth kind must answer for itself rather than falling into the
  // terminal arm.
  const paneSubject: PaneMenuSubject | null = focusedTerminal
    ? matchOwner<PaneMenuSubject>(focusedTerminal.owner, {
        session: (owner) => {
          const agent = spine?.sessions.find((s) => s.id === owner.sessionId)
          return agent
            ? { kind: "agent", session: agent }
            : {
                kind: "terminal",
                terminalId: focusedTerminal.terminalId,
                owner: focusedTerminal.owner,
              }
        },
        project: () => ({
          kind: "terminal",
          terminalId: focusedTerminal.terminalId,
          owner: focusedTerminal.owner,
        }),
        standalone: () => ({
          kind: "terminal",
          terminalId: focusedTerminal.terminalId,
          owner: focusedTerminal.owner,
        }),
      })
    : session
      ? { kind: "agent", session }
      : null

  // The width the header must hold back on its right so that whatever sits just
  // before it lands on the terminal pane's RIGHT EDGE rather than the window's.
  // A percentage, mirrored from the panel group below (see the store), because
  // the header is that group's sibling and spans the same width: no pixel is
  // measured, nothing has to know the pane's size, and it stays correct at any
  // zoom. Zero while the Changes pane is hidden, so the button slides right with
  // the terminal pane that just grew under it.
  const spacer = changesSpacerPercent(dux)

  // What the reopen control says about the pane it brings back. Null while no
  // agent is in view, which is also the state in which the phone draws no ±N
  // control at all.
  const summary = changesSummary(dux.changes, session?.id)

  return (
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
      {!changesPaneEffectivelyHidden(dux) && (
        <span
          aria-hidden="true"
          data-testid="changes-divider-continuation"
          className="pointer-events-none absolute inset-y-0 w-px bg-border"
          style={{ right: `calc(${spacer}% - ${spacer / 100}px)` }}
        />
      )}
      {/* The chips share one shrink budget so the header clips instead of
          pushing the right-hand controls off the edge: the chips yield all the
          way to nothing and the controls never move.

          The gap is wider than the header's own `gap-2` because it replaces the
          hairline dividers a field list would otherwise need; the glyphs
          already say where one field stops. One font at one size throughout,
          because the fields are peers. */}
      <div className="flex min-w-0 flex-1 items-center gap-3.5 overflow-hidden">
        {chips.map((chip) => (
          <Chip key={chip.kind} chip={chip} />
        ))}
      </div>

      {/* The macro quick-picker. It lives on the pane's own right edge, in the
          header's control family, never floating over the PTY text.
          Labelled on desktop because
          there is room and macros are a feature people forget exists; the phone
          keeps the icon variant (MobileShell), where there is not. */}
      {/* Theater first in the right-hand cluster, next to the macros trigger:
          the two are the pane's own controls, and the mode change is the one
          the eye should land on first. Same `h-8` token as its neighbours. */}
      <TheaterToggle />
      {/* The pane's own pair, rendered together or not at all: gating them
          separately lets Macros paint alone for the frame between a target
          being selected and its agent arriving in the spine, which resizes the
          cluster and shifts every control in it. */}
      {paneSubject && selectedTarget ? (
        <>
          <MacroPopover target={selectedTarget} />
          {/* The pane's top menu on a computer, the twin of the phone flap's
              `⋯`. It opens the whole menu, the same body the sidebar row's `⋯`
              opens, rather than a header-sized subset, and it sits with the
              pane's own controls rather than in the cog beside it, whose menu
              is the app's. Which body it opens is the question the header's own
              chips answer. */}
          <PaneMenu
            subject={paneSubject}
            // The pane it is painted over, which is what the INPUT group is
            // read under: a companion terminal's pane publishes under the
            // TERMINAL's id while the menu around it is the agent's.
            pane={selectedTarget}
            appearance="header"
            // No drill: the cog is mounted in this same header for as long as
            // this `⋯` exists, so drilling would offer one body twice.
            settingsDrill={false}
          />
        </>
      ) : null}

      {/* The spacer IS the control cluster, not an empty box in front of it: an
          empty spacer would push the pane's cluster left by the controls' own
          width, landing short of the divider. Sizing the cluster to the Changes
          panel's percentage and right-aligning it puts the cog on the window's
          right edge and the pane's `⋯` on the terminal pane's, out of one
          number and no measurement.

          `min-w-fit` is what makes the hidden case work: at 0% the box
          collapses to its buttons instead of crushing them, and it is the floor
          if the user drags the Changes pane narrower than they are. */}
      {/* The pane-boundary rule is a border on this cluster rather than a
          `<Separator>` in front of it, because the cluster's leading edge is
          the Changes panel's left edge. The visible divider continuation is not
          drawn here: this percentage resolves against the header's padded
          interior, so a border on this edge misses the real line. */}
      <div
        data-testid="changes-pane-spacer"
        className="flex min-w-fit shrink-0 items-center justify-end gap-2"
        style={{ width: `${spacer}%` }}
      >
        {/* The way back to a hidden Changes pane. Hiding it unmounts the pane
            (and the pane's own ⋯ menu with it), so the reopen control must live
            outside the pane: the sidebar's rail-only expand button applied to
            the right panel. Same persisted preference write as the hide item;
            outline variant so it reads as one family with the AppMenu trigger
            beside it. Desktop only by construction: InsetHeader mounts only
            in DesktopShell.

            It carries the same +/- summary the phone's changes control does,
            out of the one shared helper, because while the pane is away nothing
            else here says how much the agent has changed. The count is data
            rather than a label, so it is the deliberate exception to keeping
            transient chrome icon-only: it widens the control and leaves the
            cluster's one height token alone.

            It shows for a zero-width pane as well as a hidden one: a divider
            dragged off the edge leaves the pane at 0% with the preference still
            reading "visible", and its own hide item is inside that zero.
            `showChangesPane` restores a width as well as the preference. */}
        {changesPaneEffectivelyHidden(dux) && (
          <SimpleTooltip content="Show Changes pane">
            <Button
              variant="outline"
              size={summary ? "default" : "icon"}
              aria-label={
                summary
                  ? `Show Changes pane, ${summary.countLabel}`
                  : "Show Changes pane"
              }
              onClick={() => showChangesPane()}
            >
              <PanelRightOpen />
              {summary ? (
                <span className="tabular-nums">{summary.label}</span>
              ) : null}
            </Button>
          </SimpleTooltip>
        )}
        <AppMenu />
      </div>
    </header>
  )
}
