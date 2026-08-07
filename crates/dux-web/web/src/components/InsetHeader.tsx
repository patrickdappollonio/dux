import { Fragment } from "react"
import { PanelRightOpen } from "lucide-react"

import { AppMenu } from "@/components/AppMenu"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import { branchDrift } from "@/lib/agentTabs"
import {
  changesPaneVisible,
  setChangesPaneVisibility,
  useDux,
} from "@/lib/store"
import { matchOwner } from "@/lib/terminalOwner"
import { terminalsForOwner, terminalTitle } from "@/lib/terminals"
import type { SessionView, TerminalView } from "@/lib/types"

// One `key: value` crumb in the header details row. `muted`, when present, is
// appended as a dimmed trailing clause on the same crumb (used to surface the
// original branch when the current branch has drifted from it).
interface HeaderDetail {
  key: string
  value: string
  muted?: string
}

// The desktop center-pane top bar: a flat `key: value` list (agent, provider,
// project, branch, …) mirroring the TUI header, plus the app-menu cog.
// Extracted from App.tsx into its own module so it can be unit-tested in
// isolation without pulling in `GlobalOverlays` -> `ConfigEditorDialog`, whose
// eager Monaco import cannot initialize under vitest (see `TerminalArea`).
export function InsetHeader() {
  const dux = useDux()
  const { spine, selectedSessionId, selectedTarget } = dux
  const allTerminals = spine?.terminals ?? []
  const focusedTerminal =
    selectedTarget?.kind === "terminal" ? selectedTarget : undefined

  // The crumbs describing one AGENT: name, provider, project, branch. Shared by
  // an agent selection and by a session-owned terminal, which shows its agent's
  // crumbs and then its own.
  const agentCrumbs = (
    agent: SessionView,
    provider?: string,
  ): HeaderDetail[] => {
    const crumbs: HeaderDetail[] = [
      { key: "agent", value: agent.title || agent.branch_name },
      { key: "provider", value: provider ?? agent.provider },
    ]
    const owningProject = spine?.projects.find((p) => p.id === agent.project_id)
    if (owningProject?.name) {
      crumbs.push({ key: "project", value: owningProject.name })
    }
    // Branch crumb. When the current branch has drifted from the immutable
    // `initial_branch` the agent was created on, append a muted "originally
    // <initial>" clause so the original branch stays visible (the web has room;
    // the TUI shows a compact form only). Guarded on `initial_branch` being
    // present so an older server that omits the field never renders "originally
    // undefined".
    const drift = branchDrift(agent)
    crumbs.push({
      key: "branch",
      value: agent.branch_name,
      muted: drift.drifted ? `originally ${drift.initial}` : undefined,
    })
    return crumbs
  }

  // A focused terminal's breadcrumb is chosen by an EXHAUSTIVE match on its
  // OWNER, because the bar's whole job here is to name the thing the terminal
  // belongs to. A two-bucket lookup keeps compiling when a new kind of owner
  // arrives and hands it an empty header, which is a blank bar rather than an
  // error. Each arm answers with the owner's own crumbs plus that owner's
  // terminals: the set `terminalTitle` disambiguates against, and the set the
  // `terminals` count counts.
  const ownerContext: { crumbs: HeaderDetail[]; siblings: TerminalView[] } | null =
    focusedTerminal
      ? matchOwner(focusedTerminal.owner, {
          session: (owner) => {
            const agent = spine?.sessions.find((s) => s.id === owner.sessionId)
            return {
              crumbs: agent ? agentCrumbs(agent) : [],
              siblings: terminalsForOwner(allTerminals, owner),
            }
          },
          project: (owner) => {
            const owningProject = spine?.projects.find(
              (p) => p.id === owner.projectId,
            )
            return {
              crumbs: owningProject
                ? [{ key: "project", value: owningProject.name }]
                : [],
              siblings: terminalsForOwner(allTerminals, owner),
            }
          },
          // No owner to name, so the crumb names WHERE the terminal is. The
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
              crumbs: cwd ? [{ key: "directory", value: cwd }] : [],
              siblings,
            }
          },
        })
      : null

  // The header details, mirroring the TUI: a flat `key: value` list joined by a
  // single separator. `terminal` only appears when a companion terminal is the
  // focused target; `terminals` (the count) only when there is at least one.
  // When an agent tab is focused, the provider crumb reflects the FOCUSED TAB
  // (an extra tab can run a different provider than the session-slot tab), not
  // the session-slot tab's own provider.
  const session = spine?.sessions.find((s) => s.id === selectedSessionId)
  const focusedTabProvider =
    selectedTarget?.kind === "agent"
      ? session?.tabs.find((t) => t.id === selectedTarget.tabId)?.provider
      : undefined

  const details: HeaderDetail[] = []
  if (ownerContext && focusedTerminal) {
    details.push(...ownerContext.crumbs)
    // The crumb text is the foreground command when one is running
    // (disambiguated with the terminal's number if a sibling runs the same app),
    // otherwise the stable "Terminal N" label.
    const terminal = ownerContext.siblings.find(
      (t) => t.id === focusedTerminal.terminalId,
    )
    if (terminal) {
      details.push({
        key: "terminal",
        value: terminalTitle(terminal, ownerContext.siblings),
      })
    }
    if (ownerContext.siblings.length > 0) {
      details.push({
        key: "terminals",
        value: String(ownerContext.siblings.length),
      })
    }
  } else if (session) {
    details.push(...agentCrumbs(session, focusedTabProvider))
    const sessionTerminals = terminalsForOwner(allTerminals, {
      kind: "session",
      sessionId: session.id,
    })
    if (sessionTerminals.length > 0) {
      details.push({ key: "terminals", value: String(sessionTerminals.length) })
    }
  }

  return (
    <header className="flex h-12 shrink-0 items-center gap-2 border-b px-3">
      {/* Left region shares one shrink budget so the details row clips instead of
          pushing the right-hand controls off the edge. One font (sans) throughout
          — mixing mono values with sans labels made `items-center` misalign them
          (mono/sans glyphs center differently). Distinguish key vs value by
          color/weight, not font. See the mono/sans alignment memory. */}
      <div className="flex min-w-0 flex-1 items-center gap-x-2 overflow-hidden text-sm">
        {details.map((d, i) => (
          <Fragment key={d.key}>
            {i > 0 && (
              // A thin, vertically centered divider (items-center keeps it on the
              // text's midline — a literal "|" glyph rode high).
              <span
                aria-hidden
                className="h-3 w-px shrink-0 bg-border"
              />
            )}
            <span className="shrink-0 whitespace-nowrap">
              <span className="text-muted-foreground">{d.key}: </span>
              <span className="font-medium text-foreground">{d.value}</span>
              {d.muted ? (
                <span className="text-muted-foreground"> · {d.muted}</span>
              ) : null}
            </span>
          </Fragment>
        ))}
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
