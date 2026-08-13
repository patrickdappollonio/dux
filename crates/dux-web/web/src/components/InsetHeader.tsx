import { PanelRightOpen } from "lucide-react"

import { AppMenu } from "@/components/AppMenu"
import { MacroPopover } from "@/components/MacroPopover"
import { CHIP_GLYPHS } from "@/components/headerChipGlyphs"
import { SimpleTooltip } from "@/components/SimpleTooltip"
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
import {
  changesPaneEffectivelyHidden,
  changesSpacerPercent,
  showChangesPane,
  useDux,
} from "@/lib/store"
import { matchOwner } from "@/lib/terminalOwner"
import { terminalsForOwner, terminalTitle } from "@/lib/terminals"
import type { SessionView, TerminalView } from "@/lib/types"
import { cn } from "@/lib/utils"

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
    const owningProject = spine?.projects.find((p) => p.id === agent.project_id)
    return {
      name: agent.title || agent.branch_name,
      provider: provider ?? agent.provider,
      projectName: owningProject?.name,
      branchName: agent.branch_name,
      initialBranch: agent.initial_branch,
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
      ? matchOwner(focusedTerminal.owner, {
          session: (owner) => {
            const agent = spine?.sessions.find((s) => s.id === owner.sessionId)
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
        })
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

  return (
    <header className="flex h-12 shrink-0 items-center gap-2 border-b px-3">
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

      {/* The macro quick-picker. It used to float over the terminal as an
          absolutely-positioned overlay inside TerminalPane; it lives here now,
          on the pane's own right edge, where it sits in the header's control
          family instead of on top of the PTY text. Labelled on desktop because
          there is room and macros are a feature people forget exists; the phone
          keeps the icon variant (MobileShell), where there is not. */}
      {selectedTarget ? <MacroPopover target={selectedTarget} /> : null}

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
          (that is the whole point of sizing it to the panel's percentage), so a
          border there lands on the boundary for free. A separate element would
          sit between Macros and the cluster and collect the header's `gap-2` on
          BOTH sides, pushing Macros 9px off the pane edge it is supposed to sit
          on, which is the float this change exists to fix.

          `self-stretch` against the header's `items-center` makes it span the
          full 48px, so it and the panel divider below read as one continuous
          line; the color is the default border token, the same `--border` the
          divider's `bg-border` resolves to.

          Only while the Changes pane is actually ON SCREEN: hidden, there is
          no divider below to continue and the spacer has collapsed to the
          control cluster, so the rule would just float mid-header. The gate is
          `changesPaneEffectivelyHidden`, not the preference, so a pane the
          user dragged to nothing counts as hidden too. */}
      <div
        className={cn(
          "flex min-w-fit shrink-0 items-center justify-end gap-2",
          !changesPaneEffectivelyHidden(dux) && "self-stretch border-l",
        )}
        style={{ width: `${spacer}%` }}
      >
        {/* The way back to a hidden Changes pane. Hiding it unmounts the pane
            (and the pane's own ⋯ menu with it), so the reopen control must live
            outside the pane: the sidebar's rail-only expand button applied to
            the right panel. Same persisted preference write as the hide item;
            outline variant so it reads as one family with the AppMenu trigger
            beside it. Icon only, deliberately: unlike Macros and Settings it is
            transient chrome that exists only while the pane is away. Desktop
            only by construction: InsetHeader mounts only in DesktopShell.

            It shows for a ZERO-WIDTH pane as well, not just a hidden one. A
            divider dragged off the edge used to leave the pane at 0% with the
            preference still reading "visible", which took this button away and
            put the pane's own hide item inside the zero: nothing on screen
            could bring it back. `showChangesPane` is the healing form, which
            restores a width as well as the preference. */}
        {changesPaneEffectivelyHidden(dux) && (
          <SimpleTooltip content="Show Changes pane">
            <Button
              variant="outline"
              size="icon"
              aria-label="Show Changes pane"
              onClick={() => showChangesPane()}
            >
              <PanelRightOpen />
            </Button>
          </SimpleTooltip>
        )}
        <AppMenu />
      </div>
    </header>
  )
}
