import { PanelRightOpen } from "lucide-react"

import { AppMenu } from "@/components/AppMenu"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import {
  agentCaption,
  agentHeaderSubject,
  captionText,
  terminalCountCaption,
  terminalHeaderSubject,
  type HeaderSubject,
} from "@/lib/headerSubject"
import {
  changesPaneVisible,
  setChangesPaneVisibility,
  useDux,
} from "@/lib/store"
import { matchOwner } from "@/lib/terminalOwner"
import { terminalsForOwner, terminalTitle } from "@/lib/terminals"
import type { SessionView, TerminalView } from "@/lib/types"

// The desktop center-pane top bar: ONE SUBJECT (the agent name, or the terminal
// when a terminal is focused) with a small muted caption beside it, plus the
// app-menu cog. The decision of what belongs in each half lives in
// `lib/headerSubject.ts`, including the branch collapse.
// Extracted from App.tsx into its own module so it can be unit-tested in
// isolation without pulling in `GlobalOverlays` -> `ConfigEditorDialog`, whose
// eager Monaco import cannot initialize under vitest (see `TerminalArea`).
export function InsetHeader() {
  const dux = useDux()
  const { spine, selectedSessionId, selectedTarget } = dux
  const allTerminals = spine?.terminals ?? []
  const focusedTerminal =
    selectedTarget?.kind === "terminal" ? selectedTarget : undefined

  // The facts describing one AGENT: its name (the subject) and its caption
  // clauses. Shared by an agent selection, where the name IS the subject, and by
  // a session-owned terminal, where the terminal is the subject and the agent's
  // name joins the caption in front of the rest.
  //
  // The drift clause ("originally <initial>") is guarded inside `agentCaption`
  // on `initial_branch` being present, so an older server that omits the field
  // never renders "originally undefined".
  const agentFacts = (agent: SessionView, provider?: string) => {
    const owningProject = spine?.projects.find((p) => p.id === agent.project_id)
    return {
      name: agent.title || agent.branch_name,
      provider: provider ?? agent.provider,
      projectName: owningProject?.name,
      branchName: agent.branch_name,
      initialBranch: agent.initial_branch,
    }
  }

  // A focused terminal's caption is chosen by an EXHAUSTIVE match on its OWNER,
  // because the caption's whole job here is to name the thing the terminal
  // belongs to. A two-bucket lookup keeps compiling when a new kind of owner
  // arrives and hands it an empty header, which is a blank bar rather than an
  // error. Each arm answers with the owner's own caption clauses plus that
  // owner's terminals: the set `terminalTitle` disambiguates against, and the
  // set the sibling count counts.
  const ownerContext: { caption: string[]; siblings: TerminalView[] } | null =
    focusedTerminal
      ? matchOwner(focusedTerminal.owner, {
          session: (owner) => {
            const agent = spine?.sessions.find((s) => s.id === owner.sessionId)
            // The agent is no longer the subject here, so its NAME leads its own
            // caption clauses rather than being dropped with the labels.
            const facts = agent ? agentFacts(agent) : null
            return {
              caption: facts ? [facts.name, ...agentCaption(facts)] : [],
              siblings: terminalsForOwner(allTerminals, owner),
            }
          },
          project: (owner) => {
            const owningProject = spine?.projects.find(
              (p) => p.id === owner.projectId,
            )
            return {
              caption: owningProject ? [owningProject.name] : [],
              siblings: terminalsForOwner(allTerminals, owner),
            }
          },
          // No owner to name, so the caption names WHERE the terminal is. The
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
              caption: cwd ? [cwd] : [],
              siblings,
            }
          },
        })
      : null

  // When an agent tab is focused, the provider clause reflects the FOCUSED TAB
  // (an extra tab can run a different provider than the session-slot tab), not
  // the session-slot tab's own provider.
  const session = spine?.sessions.find((s) => s.id === selectedSessionId)
  const focusedTabProvider =
    selectedTarget?.kind === "agent"
      ? session?.tabs.find((t) => t.id === selectedTarget.tabId)?.provider
      : undefined

  let header: HeaderSubject | null = null
  if (ownerContext && focusedTerminal) {
    // The subject is the terminal itself: the foreground command when one is
    // running (disambiguated with the terminal's number if a sibling runs the
    // same app), otherwise the stable "Terminal N" label.
    const terminal = ownerContext.siblings.find(
      (t) => t.id === focusedTerminal.terminalId,
    )
    if (terminal) {
      header = terminalHeaderSubject(
        terminalTitle(terminal, ownerContext.siblings),
        ownerContext.caption,
        ownerContext.siblings.length,
      )
    }
  } else if (session) {
    const facts = agentFacts(session, focusedTabProvider)
    header = agentHeaderSubject(facts)
    const sessionTerminals = terminalsForOwner(allTerminals, {
      kind: "session",
      sessionId: session.id,
    })
    const count = terminalCountCaption(sessionTerminals.length)
    if (count) header = { ...header, caption: [...header.caption, count] }
  }

  return (
    <header className="flex h-12 shrink-0 items-center gap-2 border-b px-3">
      {/* Left region shares one shrink budget so the header clips instead of
          pushing the right-hand controls off the edge. One font (sans)
          throughout: mixing mono values with sans labels made `items-center`
          misalign them (mono/sans glyphs center differently), so the subject and
          its caption are distinguished by size, weight and color, never by font.

          The two shrink weights are the "name truncates LAST" rule. Both boxes
          are `min-w-0 truncate`, so both CAN shrink, but the caption's shrink
          factor is thousands of times the subject's: overflow is distributed in
          proportion, so the caption gives way down to nothing before the subject
          loses its first character. `items-baseline` sits the 12px caption on
          the subject's baseline rather than centering two different type sizes
          against each other. */}
      <div className="flex min-w-0 flex-1 items-baseline gap-2 overflow-hidden">
        {header ? (
          <>
            <span className="min-w-0 shrink truncate text-sm font-medium text-foreground">
              {header.subject}
            </span>
            {header.caption.length > 0 && (
              <span className="min-w-0 shrink-[9999] truncate text-xs text-muted-foreground">
                {captionText(header.caption)}
              </span>
            )}
          </>
        ) : null}
      </div>

      <div className="flex shrink-0 items-center gap-2">
        {/* The way back to a hidden Changes pane. Hiding it unmounts the pane
            (and the pane's own ⋯ menu with it), so the reopen control must live
            outside the pane — the sidebar's rail-only expand button applied to
            the right panel. Same persisted preference write as the hide item;
            outline variant so it reads as one family with the AppMenu trigger
            beside it. Desktop only by construction: InsetHeader mounts only in
            DesktopShell. */}
        {!changesPaneVisible(dux) && (
          <SimpleTooltip content="Show Changes pane">
            <Button
              variant="outline"
              size="icon"
              aria-label="Show Changes pane"
              onClick={() => setChangesPaneVisibility(true)}
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
