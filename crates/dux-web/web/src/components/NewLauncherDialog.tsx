import { Bot, ChevronRight } from "lucide-react"
import { Fragment, useMemo, useState, type KeyboardEvent } from "react"

import { IconTile } from "@/components/IconTile"
import { MatchHighlight } from "@/components/MatchHighlight"
import { Modal, ModalGroupHeading, useModalLifetime } from "@/components/modal"
import { RowStateWord } from "@/components/RowStateWord"
import { useIsMobile } from "@/hooks/use-mobile"
import {
  agentChangeBlockReason,
  useRowDriverLabel,
  useAgentChangeBlocks,
} from "@/hooks/use-agent-active-elsewhere"
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
import { FOCUS_RING, PICK_ROW } from "@/lib/focusRing"
import { matchCharRanges } from "@/lib/searchMatch"
import { closeNewLauncher, createTerminal, useDux } from "@/lib/store"
import type { RowBlock } from "@/lib/paneCover"
import type { SessionView } from "@/lib/types"
import { cn } from "@/lib/utils"

// The launcher's New dialog: every way to create something, under three
// captioned groups, each row saying in a sentence what it makes. The rows are
// `newLauncherModel`, which reads the shared creation lists, so a row runs
// exactly what the cog menu's copy of it runs.
//
// The launcher corner's menu opens the agent-picking step on its own; it then
// has no list behind it to go back to.
export function NewLauncherDialog() {
  const { newLauncherOpen, newLauncherStep } = useDux()
  const life = useModalLifetime(newLauncherOpen)
  // Keyed by the open, so each one starts on its entry step with an empty search.
  return life.mounted ? (
    <Launcher
      key={life.epoch}
      open={newLauncherOpen}
      entry={newLauncherStep}
      onClosed={life.onClosed}
    />
  ) : null
}

function Launcher({
  open,
  entry,
  onClosed,
}: {
  open: boolean
  entry: "list" | "pick-agent"
  onClosed: () => void
}) {
  const phone = useIsMobile()
  const [step, setStep] = useState(entry)
  // Coming back from the agent step puts focus back on the row that went there.
  const [returned, setReturned] = useState(false)
  const agents = useAgentPick(step === "pick-agent")
  const common = { open, onClose: closeNewLauncher, onClosed, size: "w600", step } as const
  if (step === "pick-agent") {
    return (
      <Modal
        kind="picker"
        {...common}
        title="New terminal for an agent"
        description="Which agent's copy of the code should the command line open in?"
        // Named after the dialog it returns to; absent when this step is the
        // whole dialog.
        back={
          entry === "list"
            ? {
                label: "New…",
                onSelect: () => {
                  setReturned(true)
                  setStep("list")
                },
              }
            : undefined
        }
        search={{
          value: agents.query,
          onChange: agents.setQuery,
          label: "Search agents",
          fieldProps: agents.pick.fieldProps,
        }}
      >
        <AgentRows agents={agents} />
      </Modal>
    )
  }
  return (
    <Modal
      kind="picker"
      {...common}
      title="Start something new"
      description={
        phone
          ? "Pick what you want to create."
          : "Pick what you want to create. Each one opens its own short setup."
      }
    >
      <LauncherList
        phone={phone}
        focusAgentTerminal={returned}
        onPickAgent={() => setStep("pick-agent")}
      />
    </Modal>
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
    <div className={cn("flex flex-col", !phone && "gap-1")} onKeyDown={moveFocusAmongRows}>
      {groups.map((group, index) => (
        <Fragment key={group.id}>
          {index > 0 && !phone ? (
            <div aria-hidden="true" className="mx-2 my-1 h-px bg-border" />
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
    <div
      role="group"
      aria-labelledby={headingId}
      className={cn("flex flex-col gap-0.5", phone && "pt-1.5")}
    >
      <ModalGroupHeading id={headingId} caption={group.caption}>
        {group.title}
      </ModalGroupHeading>
      {group.lead ? (
        <p className="px-2 pb-1 text-sm text-foreground">
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
      <IconTile size={phone ? "lg" : "md"} use="action">
        <Icon />
      </IconTile>
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
// agent's own menu runs.
//
// A terminal joins its agent's row, so an agent this device may not change is
// listed but cannot be picked, as its own New terminal is disabled. One nobody
// drives can, unless a pane of its row is on this page: there its card asks
// for a take-over first.
//
// `active` is false while the list step is on screen, and the agents are then not read.
function useAgentPick(active: boolean) {
  const { spine } = useDux()
  const [query, setQuery] = useState("")
  const groups = useMemo(
    () =>
      active
        ? agentTerminalGroups(
            {
              sidebar: spine?.sidebar,
              projects: spine?.projects ?? [],
              sessions: spine?.sessions ?? [],
            },
            query,
          )
        : [],
    [active, spine, query],
  )
  const sessions = groups.flatMap((group) => group.sessions)
  const blocks = useAgentChangeBlocks(sessions)
  const pickable = sessions.filter((session) => (blocks.get(session.id) ?? null) === null)
  const pickAgent = (session: SessionView) => {
    closeNewLauncher()
    createTerminal(session.id)
  }
  const pick = useSearchPick({
    query,
    items: pickable,
    keyOf: (session) => session.id,
    onPick: pickAgent,
  })
  return { query, setQuery, groups, sessions, blocks, pickable, pickAgent, pick }
}

function AgentRows({ agents }: { agents: ReturnType<typeof useAgentPick> }) {
  const { query, groups, sessions, blocks, pickable, pickAgent, pick } = agents
  const needle = query.trim().toLowerCase()
  return (
    <>
      {sessions.length > 0 && pickable.length === 0 ? (
        <p className="px-2 pt-1 text-sm text-foreground">
          None of these agents can get a terminal from this device right now;
          each one says why.
        </p>
      ) : null}
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
              <ModalGroupHeading hidden>{group.title}</ModalGroupHeading>
              {group.sessions.map((session) => (
                <AgentPickRow
                  key={session.id}
                  session={session}
                  id={pick.rowId(session.id)}
                  active={pick.shownKey === session.id}
                  needle={needle}
                  block={blocks.get(session.id) ?? null}
                  onPick={() => pickAgent(session)}
                />
              ))}
            </div>
          ))
        )}
      </div>
    </>
  )
}

function AgentPickRow({
  session,
  id,
  active,
  needle,
  block,
  onPick,
}: {
  session: SessionView
  id: string
  active: boolean
  needle: string
  block: RowBlock | null
  onPick: () => void
}) {
  const name = sessionLabel(session)
  const device = useRowDriverLabel({ sessionId: session.id })
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
      data-active={active ? "" : undefined}
      disabled={block !== null}
      onClick={onPick}
      className={cn(
        "group flex min-h-13 w-full items-center gap-3 rounded-lg border border-transparent px-2.5 py-2 text-left transition-colors hover:bg-accent focus-visible:bg-accent disabled:pointer-events-none motion-reduce:transition-none",
        FOCUS_RING,
        active && PICK_ROW,
      )}
    >
      <Bot
        aria-hidden="true"
        className="size-[18px] shrink-0 text-muted-foreground group-disabled:opacity-50"
      />
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="truncate text-sm group-disabled:opacity-50">
          <MatchHighlight text={name} ranges={matchCharRanges(name, [needle])} />
        </span>
        <span className="flex min-w-0 items-baseline gap-1.5 text-xs text-muted-foreground group-disabled:opacity-50">
          <RowStateWord key={word.label} word={word} working={working} />
          {/* The agent row's own separator: a dot, centered because a box
              with no text has no baseline. */}
          <span
            aria-hidden="true"
            className="size-1 shrink-0 self-center rounded-full bg-current opacity-50"
          />
          <AgentFolder session={session} />
        </span>
        {/* Written out at full strength rather than in a tooltip: a disabled
            button takes no pointer events and a finger has no hover. */}
        {block !== null ? (
          <span className="text-xs leading-snug text-foreground/85">
            {agentChangeBlockReason(block, device)}
          </span>
        ) : null}
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
