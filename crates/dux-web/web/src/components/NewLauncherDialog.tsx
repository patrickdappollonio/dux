import { Bot, ChevronLeft, ChevronRight, Search } from "lucide-react"
import { Fragment, useMemo, useState, type KeyboardEvent } from "react"

import { MatchHighlight } from "@/components/MatchHighlight"
import { RowStateWord } from "@/components/RowStateWord"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog"
import { Sheet, SheetContent } from "@/components/ui/sheet"
import { useIsMobile } from "@/hooks/use-mobile"
import { useSearchPick } from "@/hooks/use-search-pick"
import { agentRowVisual } from "@/lib/agentRow"
import { agentTerminalGroups } from "@/lib/agentTerminalPick"
import { matchWorkspace, sessionLabel } from "@/lib/agentWorkspace"
import {
  newLauncherContext,
  newLauncherModel,
  type NewLauncherGroup,
  type NewLauncherRow,
} from "@/lib/creationMenus"
import { stateWord } from "@/lib/flatList"
import { FOCUS_RING, ACTIVE_ROW_RING } from "@/lib/focusRing"
import { matchCharRanges } from "@/lib/searchMatch"
import { closeNewLauncher, createTerminal, useDux } from "@/lib/store"
import type { SessionView } from "@/lib/types"
import { cn } from "@/lib/utils"

// The launcher's New dialog: every way to create something, under three
// captioned groups, each row saying in a sentence what it makes. A dialog on a
// computer and a bottom sheet on a phone, around one body, so the two lists
// cannot drift. The rows are `newLauncherModel`, which reads the shared
// creation lists, so a row runs exactly what the cog menu's copy of it runs.
//
// The launcher corner's menu opens the agent-picking step on its own; it then
// has no list behind it to go back to.
export function NewLauncherDialog() {
  const { newLauncherOpen, newLauncherStep } = useDux()
  const isMobile = useIsMobile()
  const onOpenChange = (open: boolean) => {
    if (!open) closeNewLauncher()
  }
  // The body mounts only while open, so every open starts on the list with an
  // empty search rather than wherever the last one was left.
  const body = newLauncherOpen ? (
    <LauncherBody phone={isMobile} entry={newLauncherStep} />
  ) : null
  if (isMobile) {
    return (
      <Sheet open={newLauncherOpen} onOpenChange={onOpenChange}>
        <SheetContent
          side="bottom"
          className="max-h-[85dvh] gap-0 overflow-y-auto overscroll-contain rounded-t-2xl pb-[max(env(safe-area-inset-bottom),0.75rem)]"
        >
          {body}
        </SheetContent>
      </Sheet>
    )
  }
  return (
    <Dialog open={newLauncherOpen} onOpenChange={onOpenChange}>
      <DialogContent className="gap-3 p-5 sm:max-w-[600px]">{body}</DialogContent>
    </Dialog>
  )
}

function LauncherBody({
  phone,
  entry,
}: {
  phone: boolean
  entry: "list" | "pick-agent"
}) {
  const [step, setStep] = useState(entry)
  // Coming back from the agent step puts focus back on the row that went there.
  const [returned, setReturned] = useState(false)
  if (step === "pick-agent") {
    return (
      <AgentStep
        phone={phone}
        onBack={
          entry === "list"
            ? () => {
                setReturned(true)
                setStep("list")
              }
            : null
        }
      />
    )
  }
  return (
    <LauncherList
      phone={phone}
      focusAgentTerminal={returned}
      onPickAgent={() => setStep("pick-agent")}
    />
  )
}

function LauncherList({
  phone,
  focusAgentTerminal,
  onPickAgent,
}: {
  phone: boolean
  focusAgentTerminal: boolean
  onPickAgent: () => void
}) {
  const groups = newLauncherModel(newLauncherContext(useDux()))
  const activate = (row: NewLauncherRow) => {
    if (row.action.kind === "pick-agent") return onPickAgent()
    // Closed first, so the dialog the row opens is the only one on screen.
    closeNewLauncher()
    row.action.run()
  }
  return (
    <>
      <div className={cn("flex flex-col gap-1", phone ? "px-5 pt-5 pr-14" : "px-1 pr-10")}>
        <DialogTitle className="text-lg font-semibold">
          Start something new
        </DialogTitle>
        <DialogDescription>
          {phone
            ? "Pick what you want to create."
            : "Pick what you want to create. Each one opens its own short setup."}
        </DialogDescription>
      </div>
      <div
        className={cn("flex flex-col", phone ? "px-3 pt-1" : "gap-1")}
        onKeyDown={moveFocusAmongRows}
      >
        {groups.map((group, index) => (
          <Fragment key={group.id}>
            {index > 0 && !phone ? (
              <div aria-hidden="true" className="mx-1 my-1 h-px bg-border" />
            ) : null}
            <LauncherGroup
              group={group}
              phone={phone}
              focusId={focusAgentTerminal ? "new-terminal-for-agent" : null}
              onActivate={activate}
            />
          </Fragment>
        ))}
      </div>
    </>
  )
}

function LauncherGroup({
  group,
  phone,
  focusId,
  onActivate,
}: {
  group: NewLauncherGroup
  phone: boolean
  focusId: string | null
  onActivate: (row: NewLauncherRow) => void
}) {
  const headingId = `new-launcher-${group.id}`
  return (
    <div role="group" aria-labelledby={headingId} className="flex flex-col gap-0.5">
      <div
        className={cn(
          "flex gap-x-2 gap-y-0.5 px-1",
          phone ? "flex-col px-2 pt-3 pb-1" : "flex-wrap items-baseline pt-1.5 pb-1",
        )}
      >
        <span
          id={headingId}
          className="text-xs font-semibold tracking-wide text-foreground/85 uppercase"
        >
          {group.title}
        </span>
        <span className="text-xs text-muted-foreground">{group.caption}</span>
      </div>
      {group.lead ? (
        <p className={cn("text-sm text-foreground", phone ? "px-2 pb-1" : "px-1 pb-1")}>
          {group.lead}
        </p>
      ) : null}
      {group.rows.map((row) => (
        <LauncherRowButton
          key={row.id}
          row={row}
          phone={phone}
          autoFocus={row.id === focusId}
          onActivate={() => onActivate(row)}
        />
      ))}
    </div>
  )
}

function LauncherRowButton({
  row,
  phone,
  autoFocus,
  onActivate,
}: {
  row: NewLauncherRow
  phone: boolean
  autoFocus: boolean
  onActivate: () => void
}) {
  const Icon = row.icon
  return (
    <button
      type="button"
      data-launcher-row=""
      data-row-id={row.id}
      disabled={row.disabled}
      autoFocus={autoFocus}
      onClick={onActivate}
      // The rows sit 2px apart with nothing else interactive between them, and
      // each is at least 56px tall, well over the touch floor.
      className={cn(
        "flex w-full items-center gap-3 rounded-lg border border-transparent text-left transition-colors hover:bg-accent focus-visible:bg-accent disabled:pointer-events-none disabled:opacity-50 motion-reduce:transition-none",
        phone ? "min-h-16 px-2 py-2.5" : "min-h-14 px-2.5 py-2",
        FOCUS_RING,
      )}
    >
      <span
        className={cn(
          "flex shrink-0 items-center justify-center rounded-lg border border-border/60 bg-muted text-foreground/90",
          phone ? "size-10" : "size-9",
        )}
      >
        <Icon className={phone ? "size-5" : "size-[18px]"} />
      </span>
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className={cn("font-medium", phone ? "text-[15px]" : "text-sm")}>
          {row.title}
        </span>
        <span
          className={cn(
            "leading-snug text-muted-foreground",
            phone ? "text-[13px]" : "text-[12.5px]",
          )}
        >
          {row.excerpt}
        </span>
      </span>
      {row.opensStep && !phone ? (
        <ChevronRight
          aria-hidden="true"
          className="size-4 shrink-0 text-muted-foreground"
        />
      ) : null}
    </button>
  )
}

// Up and Down walk the rows the way a menu's arrows do, skipping the disabled
// ones, and Home and End jump to either end. Tab still leaves the list.
function moveFocusAmongRows(event: KeyboardEvent<HTMLElement>) {
  const keys = ["ArrowDown", "ArrowUp", "Home", "End"]
  if (!keys.includes(event.key)) return
  if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return
  const rows = Array.from(
    event.currentTarget.querySelectorAll<HTMLButtonElement>(
      "button[data-launcher-row]:not(:disabled)",
    ),
  )
  if (rows.length === 0) return
  const at = rows.indexOf(document.activeElement as HTMLButtonElement)
  const next =
    event.key === "Home"
      ? 0
      : event.key === "End"
        ? rows.length - 1
        : event.key === "ArrowDown"
          ? Math.min(at + 1, rows.length - 1)
          : at < 0
            ? rows.length - 1
            : Math.max(at - 1, 0)
  event.preventDefault()
  rows[next]?.focus()
}

// The second step behind "New terminal for an agent": which agent's directory
// the terminal opens in. Picking one opens it through the same action the
// agent's own menu runs. `onBack` is null when the step is the whole dialog.
function AgentStep({
  phone,
  onBack,
}: {
  phone: boolean
  onBack: (() => void) | null
}) {
  const { spine } = useDux()
  const [query, setQuery] = useState("")
  const groups = useMemo(
    () =>
      agentTerminalGroups(
        {
          sidebar: spine?.sidebar,
          projects: spine?.projects ?? [],
          sessions: spine?.sessions ?? [],
        },
        query,
      ),
    [spine, query],
  )
  const sessions = groups.flatMap((group) => group.sessions)
  const pickAgent = (session: SessionView) => {
    closeNewLauncher()
    createTerminal(session.id)
  }
  const pick = useSearchPick({
    query,
    items: sessions,
    keyOf: (session) => session.id,
    onPick: pickAgent,
  })
  const needle = query.trim().toLowerCase()
  return (
    <>
      <div
        className={cn(
          "flex flex-col gap-1",
          phone ? "px-3 pr-14" : "pr-10",
          phone ? (onBack ? "pt-3" : "pt-5") : onBack ? "-mt-1" : null,
        )}
      >
        {/* Back to the list, named after the dialog it returns to. Ghost and
            quieter than the rows: it navigates rather than creates. 32px tall
            on a computer, where its only neighbour is the close button across
            the header; the floor returns where a finger is the pointer. */}
        {onBack ? (
          <Button
            variant="ghost"
            size="sm"
            onClick={onBack}
            className="h-8 self-start px-1.5 text-muted-foreground max-md:min-h-10 pointer-coarse:min-h-10"
          >
            <ChevronLeft />
            New…
          </Button>
        ) : null}
        <div className={cn("flex flex-col gap-1", phone ? "px-2" : "px-1")}>
          <DialogTitle className="text-lg font-semibold">
            New terminal for an agent
          </DialogTitle>
          <DialogDescription>
            Which agent's copy of the code should the command line open in?
          </DialogDescription>
        </div>
      </div>
      <div className={cn("flex flex-col gap-2", phone ? "px-3 pt-2" : "")}>
        <div className="flex items-center gap-2 rounded-md border border-input bg-input/30 px-3 max-md:min-h-10">
          <Search className="size-4 shrink-0 text-muted-foreground" />
          <input
            type="search"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Search agents"
            aria-label="Search agents"
            {...pick.fieldProps}
            className="min-w-0 flex-1 bg-transparent py-2 text-sm outline-none placeholder:text-muted-foreground"
            autoFocus
          />
        </div>
        <div className="flex flex-col gap-0.5" onKeyDown={moveFocusAmongRows}>
          {sessions.length === 0 ? (
            <p className="px-2 py-4 text-sm text-muted-foreground">
              {needle === "" ? "No agents yet." : "No agents match that search."}
            </p>
          ) : (
            groups.map((group) => (
              <div
                key={group.id}
                role="group"
                aria-label={group.title}
                className="flex flex-col gap-0.5"
              >
                <span
                  aria-hidden="true"
                  className="px-2 pt-2 pb-0.5 text-xs font-semibold tracking-wide text-foreground/85 uppercase"
                >
                  {group.title}
                </span>
                {group.sessions.map((session) => (
                  <AgentPickRow
                    key={session.id}
                    session={session}
                    id={pick.rowId(session.id)}
                    active={pick.activeKey === session.id}
                    needle={needle}
                    onPick={() => pickAgent(session)}
                  />
                ))}
              </div>
            ))
          )}
        </div>
      </div>
    </>
  )
}

function AgentPickRow({
  session,
  id,
  active,
  needle,
  onPick,
}: {
  session: SessionView
  id: string
  active: boolean
  needle: string
  onPick: () => void
}) {
  const name = sessionLabel(session)
  const word = stateWord(session)
  // The same flag the sidebar row's cue fires on, so the word pulses and
  // carries its ellipsis exactly when it reads the busy word.
  const { working } = agentRowVisual(
    session.status,
    session.working,
    session.needs_attention,
    session.typing,
  )
  return (
    <button
      type="button"
      id={id}
      data-launcher-row=""
      onClick={onPick}
      className={cn(
        "flex min-h-13 w-full items-center gap-3 rounded-lg border border-transparent px-2.5 py-2 text-left transition-colors hover:bg-accent focus-visible:bg-accent motion-reduce:transition-none",
        FOCUS_RING,
        active && ACTIVE_ROW_RING,
      )}
    >
      <Bot aria-hidden="true" className="size-[18px] shrink-0 text-muted-foreground" />
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="truncate text-sm">
          <MatchHighlight text={name} ranges={matchCharRanges(name, [needle])} />
        </span>
        <span className="flex min-w-0 items-baseline gap-1.5 text-xs text-muted-foreground">
          <RowStateWord key={word.label} word={word} working={working} />
          {/* The agent row's own separator: a dot, centered because a box
              with no text has no baseline. */}
          <span
            aria-hidden="true"
            className="size-1 shrink-0 self-center rounded-full bg-current opacity-50"
          />
          <AgentFolder session={session} />
        </span>
      </span>
    </button>
  )
}

// Where the terminal will open, home-collapsed: the worktree, or the standalone
// agent's own folder behind the standalone star.
function AgentFolder({ session }: { session: SessionView }) {
  return matchWorkspace(session.workspace, {
    managed: (w) => (
      <span className="min-w-0 truncate font-mono text-[11.5px]">
        {w.worktree_label ?? w.worktree_path}
      </span>
    ),
    folder: (w) => (
      <span className="flex min-w-0 items-baseline gap-1 font-mono text-[11.5px] text-dux-standalone">
        <span aria-hidden="true">✷</span>
        <span className="sr-only">standalone</span>
        <span className="min-w-0 truncate">{w.folder_label}</span>
      </span>
    ),
  })
}
