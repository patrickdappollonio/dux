import { Bot, Check, Ellipsis, Plus, Replace, X } from "lucide-react"

import { SimpleTooltip } from "@/components/SimpleTooltip"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { tabLabels } from "@/lib/agentTabs"
import {
  addTab,
  openCloseTab,
  retargetTab,
  selectTab,
  useDux,
} from "@/lib/store"
import { DEFAULT_AGENT_TABS_MAX } from "@/lib/bootstrapApi"
import { cn } from "@/lib/utils"
import type { AgentTabView, SessionView } from "@/lib/types"

// The Chrome-style provider-tab strip at the top of the center pane, rendered only
// when a session has two or more tabs (App gates this). All tabs are generic and
// render uniformly. Clicking a pill focuses it; the hover-revealed ⋯ menu retargets
// the provider and closes the tab; the trailing + adds a tab (disabled at the cap or
// while a create is in flight). This is web-only chrome; the TUI has its own themed
// strip.
export function AgentTabsStrip({
  session,
  activeTabId,
  maxTabs,
}: {
  session: SessionView
  activeTabId: string
  maxTabs?: number
}) {
  const { bootstrap, createTabInFlight } = useDux()
  const providers = bootstrap?.available_providers ?? []
  const labels = tabLabels(session.tabs)
  const cap = maxTabs ?? DEFAULT_AGENT_TABS_MAX
  const atCap = session.tabs.length >= cap
  const creating = createTabInFlight.includes(session.id)

  return (
    <div className="flex items-center gap-1 overflow-x-auto border-b bg-muted/30 px-2 py-1">
      {session.tabs.map((tab, i) => (
        <TabPill
          key={tab.id}
          session={session}
          tab={tab}
          label={labels[i]}
          active={tab.id === activeTabId}
          providers={providers}
        />
      ))}
      <SimpleTooltip
        content={
          atCap ? `Tab limit reached (${cap})` : creating ? "Adding…" : "New tab"
        }
      >
        <button
          type="button"
          aria-label="New tab"
          disabled={atCap || creating}
          onClick={() => addTab(session.id)}
          className={cn(
            "flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-background hover:text-foreground disabled:pointer-events-none disabled:opacity-40 max-md:size-11",
          )}
        >
          <Plus className="size-4" />
        </button>
      </SimpleTooltip>
    </div>
  )
}

function TabPill({
  session,
  tab,
  label,
  active,
  providers,
}: {
  session: SessionView
  tab: AgentTabView
  label: string
  active: boolean
  providers: string[]
}) {
  function select() {
    selectTab(session.id, tab.id)
  }

  return (
    <div
      role="tab"
      aria-selected={active}
      tabIndex={0}
      onClick={select}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault()
          select()
        }
      }}
      className={cn(
        "group/tab flex shrink-0 cursor-pointer items-center gap-1.5 rounded-md border px-2 py-1 text-sm transition-colors max-md:min-h-11",
        active
          ? "border-border bg-background text-foreground"
          : "border-transparent bg-muted text-muted-foreground hover:text-foreground",
      )}
    >
      <Bot
        className={cn(
          "size-3.5 shrink-0 motion-safe:transition-transform motion-safe:duration-300",
          tab.working && "motion-safe:animate-agent-working",
        )}
      />
      <span className="max-w-40 truncate">{label}</span>
      <DropdownMenu>
        <DropdownMenuTrigger
          render={
            <button
              type="button"
              aria-label="Tab actions"
              onClick={(e) => e.stopPropagation()}
              className="flex size-5 shrink-0 items-center justify-center rounded text-muted-foreground opacity-0 transition-opacity hover:text-foreground group-hover/tab:opacity-100 focus-visible:opacity-100 data-[popup-open]:opacity-100 max-md:size-11 max-md:opacity-100"
            />
          }
        >
          <Ellipsis className="size-3.5" />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" onClick={(e) => e.stopPropagation()}>
          <DropdownMenuSub>
            <DropdownMenuSubTrigger>
              <Replace />
              Change provider…
            </DropdownMenuSubTrigger>
            <DropdownMenuSubContent>
              {providers.map((p) => {
                const current = p === tab.provider
                return (
                  <DropdownMenuItem
                    key={p}
                    disabled={current}
                    onClick={() => void retargetTab(session.id, tab.id, p)}
                  >
                    {current ? <Check /> : <Bot />}
                    {p}
                  </DropdownMenuItem>
                )
              })}
            </DropdownMenuSubContent>
          </DropdownMenuSub>
          <DropdownMenuSeparator />
          <DropdownMenuItem onClick={() => openCloseTab(session.id, tab.id)}>
            <X />
            Close tab…
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  )
}
