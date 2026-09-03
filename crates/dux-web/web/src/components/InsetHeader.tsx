import { PanelRightOpen } from "lucide-react"

import { AppMenu } from "@/components/AppMenu"
import { MacroPopover } from "@/components/MacroPopover"
import { PaneMenu } from "@/components/PaneMenu"
import { CHIP_GLYPHS } from "@/components/headerChipGlyphs"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { TheaterToggle } from "@/components/TheaterToggle"
import { Button } from "@/components/ui/button"
import { useIsTruncated } from "@/hooks/use-truncated"
import {
  agentHeaderChips,
  directoryChip,
  focusedTerminalChip,
  headerChipTooltip,
  type AgentChipsInput,
  type HeaderChip,
} from "@/lib/headerSubject"
import { changesSummary } from "@/lib/changesSummary"
import {
  changesPaneEffectivelyHidden,
  changesSpacerPercent,
  showChangesPane,
  useDux,
} from "@/lib/store"
import { matchOwner } from "@/lib/terminalOwner"
import { terminalsForOwner, terminalTitle } from "@/lib/terminals"
import type { SessionView, TerminalView } from "@/lib/types"
import {
  managedWorkspace,
  sessionLabel,
  workspaceLocation,
} from "@/lib/agentWorkspace"

// The desktop center-pane top bar: ONE ROW OF CHIPS naming what you are looking
// at, each a glyph followed by its value, then the pane's controls on the right.
// WHICH chips exist and what each says lives in `lib/headerSubject.ts`; this
// module is how they are drawn.
//
// Extracted from App.tsx into its own module so it can be unit-tested in
// isolation without pulling in `GlobalOverlays` -> `ConfigEditorDialog`, whose
// eager Monaco import cannot initialize under vitest (see `TerminalArea`).

// One glyph-and-value pair.
//
// The two shrink weights are the "the name gives way LAST" rule. Every chip is
// `min-w-0` and can shrink, but a non-primary chip's shrink factor is thousands
// of times the primary one's, so overflow is distributed in proportion and the
// rest of the row yields to nothing before the thing you navigate by loses its
// first character. `overflow-hidden` on the chip is what lets the glyph clip
// too: the chips yield ALL the way, and a lone floating glyph with no value
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
  const allTerminals = spine?.terminals ?? []
  const focusedTerminal =
    selectedTarget?.kind === "terminal" ? selectedTarget : undefined

  // The facts describing one AGENT. Shared by an agent selection, where the
  // agent's own name is the primary chip, and by a session-owned terminal, where
  // the terminal is primary and the agent's chips sit in front of it.
  const agentFacts = (
    agent: SessionView,
    provider: string | undefined,
    terminalCount: number,
    primary: "agent" | "none",
  ): AgentChipsInput => {
    // A standalone agent has no project; it names its FOLDER in the same slot
    // instead, through the chip a standalone terminal already uses.
    const location = workspaceLocation(agent.workspace)
    const owningProject =
      location.kind === "project"
        ? spine?.projects.find((p) => p.id === location.projectId)
        : undefined
    const managed = managedWorkspace(agent.workspace)
    return {
      name: sessionLabel(agent),
      provider: provider ?? agent.provider,
      projectName: owningProject?.name,
      folderLabel: location.kind === "folder" ? location.label : undefined,
      branchName: managed?.branch_name ?? null,
      initialBranch: managed?.initial_branch ?? null,
      terminalCount,
      primary,
    }
  }

  // A focused terminal's context chips are chosen by an EXHAUSTIVE match on its
  // OWNER, because their whole job is to name the thing the terminal belongs to.
  // A two-bucket lookup keeps compiling when a new kind of owner arrives and
  // hands it an empty header, which is a blank bar rather than an error. Each
  // arm answers with the owner's own chips plus that owner's terminals: the set
  // `terminalTitle` disambiguates against, and the set the sibling count counts.
  const ownerContext: { chips: HeaderChip[]; siblings: TerminalView[] } | null =
    focusedTerminal
      ? matchOwner<{ chips: HeaderChip[]; siblings: TerminalView[] }>(
          focusedTerminal.owner,
          {
            session: (owner) => {
              const agent = spine?.sessions.find(
                (s) => s.id === owner.sessionId,
              )
              const siblings = terminalsForOwner(allTerminals, owner)
              // The agent is no longer the primary chip here, and its terminal
              // COUNT is suppressed: the focused terminal's own chip carries that
              // count in its hover clause, and two terminal glyphs in one row
              // would read as two different terminals.
              return {
                chips: agent
                  ? agentHeaderChips(agentFacts(agent, undefined, 0, "none"))
                  : [],
                siblings,
              }
            },
            project: (owner) => {
              const owningProject = spine?.projects.find(
                (p) => p.id === owner.projectId,
              )
              return {
                chips: owningProject
                  ? [
                      {
                        kind: "project" as const,
                        label: "Project",
                        value: owningProject.name,
                      },
                    ]
                  : [],
                siblings: terminalsForOwner(allTerminals, owner),
              }
            },
            // No owner to name, so the context names WHERE the terminal is. The
            // label is read off this terminal's own wire owner rather than the
            // client-side reference, which carries no id and no label precisely
            // because there is no owner: every standalone terminal shares one
            // reference, and only the terminal itself knows its directory.
            standalone: (owner) => {
              const siblings = terminalsForOwner(allTerminals, owner)
              const self = siblings.find(
                (t) => t.id === focusedTerminal.terminalId,
              )
              const cwd =
                self?.owner.kind === "standalone" ? self.owner.cwd_label : null
              return {
                chips: cwd ? [directoryChip(cwd)] : [],
                siblings,
              }
            },
          },
        )
      : null

  // When an agent tab is focused, the assistant chip reflects the FOCUSED TAB
  // (an extra tab can run a different provider than the session-slot tab), not
  // the session-slot tab's own provider.
  const session = spine?.sessions.find((s) => s.id === selectedSessionId)
  const focusedTabProvider =
    selectedTarget?.kind === "agent"
      ? session?.tabs.find((t) => t.id === selectedTarget.tabId)?.provider
      : undefined

  let chips: HeaderChip[] = []
  if (ownerContext && focusedTerminal) {
    const terminal = ownerContext.siblings.find(
      (t) => t.id === focusedTerminal.terminalId,
    )
    if (terminal) {
      // The terminal chip lands where a terminal chip always lands: after the
      // branch, before the assistant. Its value is the terminal's own title
      // rather than a count, because the terminal is the thing on screen.
      const self = focusedTerminalChip(
        terminalTitle(terminal, ownerContext.siblings),
        ownerContext.siblings.length,
      )
      const before = ownerContext.chips.filter((c) => c.kind !== "assistant")
      const after = ownerContext.chips.filter((c) => c.kind === "assistant")
      chips = [...before, self, ...after]
    }
  } else if (session) {
    const sessionTerminals = terminalsForOwner(allTerminals, {
      kind: "session",
      sessionId: session.id,
    })
    chips = agentHeaderChips(
      agentFacts(
        session,
        focusedTabProvider,
        sessionTerminals.length,
        "agent",
      ),
    )
  }

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
          positioned on purpose, and this replaced a border-l on the control
          cluster: the cluster's width percentage resolves against the header's
          PADDED interior while the panel divider below sits at the same
          percentage of the FULL width, so the border drew a few pixels left of
          the line it claims to continue (reported from a real screenshot). An
          absolute offset resolves against the header's full box, so this
          hairline lands on the divider at every split, width, and zoom. It is
          also outside the flex flow, so it collects no `gap-2` and cannot push
          the macro button off the pane edge. Same visibility rule as before:
          only while the Changes pane is actually on screen, because hidden
          there is no divider below to continue.

          The calc's pixel term compensates for the panel HANDLE: the group
          lays out as terminal, a 1px handle, changes, so the two panes split
          the full width MINUS that pixel and a pure percentage of the full
          width lands spacer/100 px left of the real divider. Sub-pixel at
          100 percent zoom, a visible one-pixel step under browser zoom
          (reported from a zoomed screenshot). */}
      {!changesPaneEffectivelyHidden(dux) && (
        <span
          aria-hidden="true"
          data-testid="changes-divider-continuation"
          className="pointer-events-none absolute inset-y-0 w-px bg-border"
          style={{ right: `calc(${spacer}% - ${spacer / 100}px)` }}
        />
      )}
      {/* The chips share one shrink budget so the header clips instead of
          pushing the right-hand controls off the edge. Buttons win: the chips
          yield, all the way to nothing, and the controls never move or reflow.
          The header stays exactly 48px at every width.

          The gap is wider than the header's own `gap-2` on purpose. There are
          deliberately NO hairline dividers between the fields: a glyph already
          announces where one field stops and the next begins, so a rule would
          only spend pixels, and the wider gap does the same work for free. One
          font (sans) throughout, at one size: the fields are peers, so nothing
          here is distinguished by font or by type scale. */}
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
      {selectedTarget ? <MacroPopover target={selectedTarget} /> : null}
      {/* THE PANE'S TOP MENU on a computer, the twin of the phone flap's `⋯`.
          It opens the WHOLE agent menu, the same body the sidebar row's `⋯`
          opens, rather than a header-sized subset: a pane in front of you and
          a row in a list are two anchors on one agent, and two menus about the
          same agent are two things that can disagree. It sits with the pane's
          own controls, on the pane's right edge, and not in the cog beside it,
          because the cog's menu is the app's and this one is about one pane.

          Only for an agent: a focused terminal's own menu arrives with the
          terminal treatment. */}
      {session && selectedTarget?.kind === "agent" ? (
        <PaneMenu session={session} appearance="header" />
      ) : null}

      {/* The spacer IS the control cluster, rather than an empty box in front of
          it, and that is the whole trick. An empty spacer followed by the
          controls would push Macros left by the controls' own width, so it
          would land well short of the divider and only pixel math could correct
          it. Sizing the cluster itself to the Changes panel's percentage and
          right-aligning its contents puts the cog on the window's right edge and
          Macros on the terminal pane's, out of one number and no measurement.

          `min-w-fit` is what makes the hidden case work: at 0% the box collapses
          to its buttons instead of crushing them, so Macros simply slides right
          with the terminal pane that just grew under it. It is also the floor
          that keeps the buttons intact if the user drags the Changes pane
          narrower than they are. */}
      {/* The rule at the pane boundary. It is a BORDER ON THE CLUSTER rather
          than a `<Separator>` element in front of it, and that is deliberate:
          the cluster's leading edge is exactly the Changes panel's left edge
          (that is the whole point of sizing it to the panel's percentage).
          The visible divider continuation is NOT drawn here though: this
          cluster's percentage resolves against the header's padded interior,
          which is a few pixels narrower than the panel group below, so a
          border on this edge misses the real divider line. The hairline at
          the top of the header (absolutely positioned, full-box percentage)
          owns that job; see its comment. */}
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
            else on this surface says how much the agent has changed. The count
            is DATA rather than a label, so it is the deliberate exception to
            keeping transient chrome icon-only: it widens the control and leaves
            the cluster's one height token alone. With no agent in view there is
            nothing for a count to be about, and the control is the bare icon it
            has always been, which is also what the phone draws there.

            It shows for a ZERO-WIDTH pane as well, not just a hidden one. A
            divider dragged off the edge leaves the pane at 0% with the
            preference still reading "visible"; if this button hid then,
            nothing on screen could bring the pane back (its own hide item is
            inside the zero). `showChangesPane` is the healing form, which
            restores a width as well as the preference. */}
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
