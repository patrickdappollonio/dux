import { Fragment } from "react"

import { AppMenu } from "@/components/AppMenu"
import { branchDrift } from "@/lib/agentTabs"
import { useDux } from "@/lib/store"
import { terminalTitle } from "@/lib/terminals"

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
  const { spine, selectedSessionId, selectedTarget } = useDux()
  const session = spine?.sessions.find((s) => s.id === selectedSessionId)
  const project = session
    ? spine?.projects.find((p) => p.id === session.project_id)
    : undefined
  // When a companion terminal is focused, surface it as a third crumb. The crumb
  // text is the foreground command when one is running (disambiguated with the
  // terminal's number if a sibling runs the same app), otherwise the stable
  // "Terminal N" label.
  const terminal =
    selectedTarget?.kind === "terminal"
      ? session?.terminals.find((t) => t.id === selectedTarget.terminalId)
      : undefined
  const terminalLabel =
    terminal && session
      ? terminalTitle(terminal, session.terminals)
      : undefined

  // The header details, mirroring the TUI: a flat `key: value` list joined by a
  // single separator. `terminal` only appears when a companion terminal is the
  // focused target; `terminals` (the count) only when there is at least one.
  // When an agent tab is focused, the provider crumb reflects the FOCUSED TAB
  // (an extra tab can run a different provider than the session-slot tab), not
  // the session-slot tab's own provider.
  const focusedTabProvider =
    selectedTarget?.kind === "agent"
      ? session?.tabs.find((t) => t.id === selectedTarget.tabId)?.provider
      : undefined

  const details: HeaderDetail[] = []
  if (session) {
    details.push({ key: "agent", value: session.title || session.branch_name })
    details.push({
      key: "provider",
      value: focusedTabProvider ?? session.provider,
    })
    if (project?.name) details.push({ key: "project", value: project.name })
    // Branch crumb. When the current branch has drifted from the immutable
    // `initial_branch` the agent was created on, append a muted "originally
    // <initial>" clause so the original branch stays visible (the web has room;
    // the TUI shows a compact form only). Guarded on `initial_branch` being
    // present so an older server that omits the field never renders "originally
    // undefined".
    const drift = branchDrift(session)
    details.push({
      key: "branch",
      value: session.branch_name,
      muted: drift.drifted ? `originally ${drift.initial}` : undefined,
    })
    if (terminalLabel) details.push({ key: "terminal", value: terminalLabel })
    if (session.terminals.length > 0) {
      details.push({ key: "terminals", value: String(session.terminals.length) })
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
        <AppMenu />
      </div>
    </header>
  )
}
