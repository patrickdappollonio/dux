import { Bot, Check, ChevronDown, Ellipsis, Plus, Replace, X } from "lucide-react"

import { SimpleTooltip } from "@/components/SimpleTooltip"
import { AttentionDot } from "@/components/AttentionDot"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import {
  ONLY_TAB_CLOSE_REFUSAL,
  defaultProviderForSession,
  tabLabels,
} from "@/lib/agentTabs"
import {
  addTab,
  openCloseTab,
  retargetTab,
  selectTab,
  useDux,
} from "@/lib/store"
import { DEFAULT_AGENT_TABS_MAX } from "@/lib/bootstrapApi"
import { ALWAYS_REVEALED_ON_TOUCH } from "@/lib/touchReveal"
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
  const { bootstrap, spine, createTabInFlight } = useDux()
  const providers = bootstrap?.available_providers ?? []
  const labels = tabLabels(session.tabs)
  const cap = maxTabs ?? DEFAULT_AGENT_TABS_MAX
  const atCap = session.tabs.length >= cap
  const creating = createTabInFlight.includes(session.id)
  const defaultProvider = defaultProviderForSession(spine, session)
  const disabled = atCap || creating

  return (
    // `max-md:py-0.5` halves the strip's own vertical padding to go with the
    // shorter phone pill below: shrinking the pill alone would leave the strip
    // the same height. Desktop padding is unchanged.
    <div className="flex items-center gap-1 overflow-x-auto border-b bg-muted/30 px-2 py-1 max-md:py-0.5">
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
      {/* Split "+" control: the main button quick-adds the project default
          provider (today's behavior, unchanged); the adjacent caret opens a
          menu to pick a different configured provider. Misclick-safe spacing
          between the two halves mirrors the per-pill ⋯ menu's gap conventions. */}
      <div className="flex shrink-0 items-center gap-0.5">
        <SimpleTooltip
          content={
            atCap
              ? `Tab limit reached (${cap})`
              : creating
                ? "Adding…"
                : `New ${defaultProvider} tab`
          }
        >
          <button
            type="button"
            aria-label="New tab"
            disabled={disabled}
            onClick={() => addTab(session.id)}
            // `max-md:h-9 max-md:w-11` rather than `max-md:size-11`: the strip's
            // height relaxation is VERTICAL only (see the pill below), so this
            // keeps its full 44px WIDTH, where its neighbour is the provider
            // caret and a stray sideways tap would add the wrong provider.
            className="flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-background hover:text-foreground disabled:pointer-events-none disabled:opacity-40 max-md:h-9 max-md:w-11"
          >
            <Plus className="size-4" />
          </button>
        </SimpleTooltip>
        <DropdownMenu>
          <DropdownMenuTrigger
            render={
              <button
                type="button"
                aria-label="Choose provider for new tab"
                disabled={disabled}
                // Height relaxed, width kept: same reasoning as the "+" beside
                // it.
                className="flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-background hover:text-foreground disabled:pointer-events-none disabled:opacity-40 max-md:h-9 max-md:w-11"
              />
            }
          >
            <ChevronDown className="size-3.5" />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            {providers.map((p) => {
              const isDefault = p === defaultProvider
              return (
                <DropdownMenuItem key={p} onClick={() => addTab(session.id, p)}>
                  {isDefault ? <Check /> : <Bot />}
                  {p}
                  {isDefault ? (
                    <span className="ml-auto text-xs text-muted-foreground">
                      default
                    </span>
                  ) : null}
                </DropdownMenuItem>
              )
            })}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
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
  // No successor means no slot to hand on, which is the one close the server
  // refuses. Answered from the tab list rather than from slot-ness, because
  // "this is the first tab" and "this is the only tab" are different facts and
  // only the second one refuses.
  const soleTab = session.tabs.length <= 1

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
      // `max-md:min-h-9` (36px) is a deliberate, per-axis relaxation of the 40px
      // touch-target floor, taken under the tenet's exemption and justified by
      // naming the neighbours. The relaxed axis is VERTICAL: above the strip is
      // the mobile header, whose own controls end at its bottom edge and which
      // offers no tap target adjacent to a pill; below is the PTY. The PTY is
      // not inert (a tap there focuses the compose box, and with mouse tracking
      // on it forwards a click to the app), but both are CHEAP mis-taps: a
      // keyboard you dismiss, or a click the app ignores. Nothing here is
      // destructive and nothing switches what you are looking at. HORIZONTALLY
      // the pill keeps its size, because its neighbours are OTHER TABS and
      // landing on the wrong tab is a real mis-tap. The strip sits between the
      // header and the terminal, where vertical space is the scarce resource on
      // a phone.
      className={cn(
        // `max-md:py-0` goes with it: the pill's own 4px padding would sit on
        // top of the 32px ⋯ hit area inside and overshoot the 36px again.
        "group/tab flex shrink-0 cursor-pointer items-center gap-1.5 rounded-md border px-2 py-1 text-sm transition-colors max-md:min-h-9 max-md:py-0",
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
      {/* Cyan attention dot on the flagged tab's pill (a permission prompt or a
          finished turn on this specific tab). */}
      {tab.needs_attention && <AttentionDot />}
      <span className="max-w-40 truncate">{label}</span>
      {/* The ⋯ trigger consumes NO layout space at rest: the wrapper's max-width
          collapses to zero (not opacity-only, which would still reserve the
          fixed-size box's width) and animates open on hover, focus-within, or
          while the menu is open (trigger `data-popup-open`, which Base UI does
          NOT mirror onto `aria-expanded`) — mirroring ChangedFiles.tsx/Sidebar.tsx.
          Always revealed on touch. */}
      <div
        className={cn(
          "flex shrink-0 items-center overflow-hidden transition-[max-width,opacity] duration-200 ease-out max-md:max-w-none motion-reduce:transition-none max-w-0 opacity-0 group-hover/tab:max-w-8 group-hover/tab:opacity-100 group-focus-within/tab:max-w-8 group-focus-within/tab:opacity-100 has-[[data-popup-open]]:max-w-8 has-[[data-popup-open]]:opacity-100",
          ALWAYS_REVEALED_ON_TOUCH,
        )}
      >
        <DropdownMenu>
          <DropdownMenuTrigger
            render={
              <button
                type="button"
                aria-label="Tab actions"
                onClick={(e) => e.stopPropagation()}
                // `max-md:h-8 max-md:w-11` keeps the full 44px width (its
                // horizontal neighbour is the next tab) while fitting inside the
                // pill's 36px height; `max-md:size-11` here would have forced
                // the pill back to 44px and undone the shorter strip.
                className="flex size-5 shrink-0 items-center justify-center rounded text-muted-foreground hover:text-foreground max-md:h-8 max-md:w-11"
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
            {/* Every tab closes the same way, the agent's FIRST included: the
                slot is a pointer, so closing the tab holding it hands the slot
                to the next tab in strip order rather than being refused. The
                confirmation dialog is where the difference is spelled out.

                The exception is an agent's ONLY tab, which has no successor to
                hand the slot to and which the server refuses with the sentence
                shown here. The browser refuses it in the same place the
                terminal UI does, BEFORE any dialog: walking a user through a
                confirmation into a 400 promises a detach that was never going
                to happen. The item stays and renders disabled (a missing item
                reads as a bug, a dimmed one reads as a rule), with the reason
                as a menu label above it: a disabled item is
                `pointer-events-none`, so a tooltip hung on it could never be
                opened, by a pointer or by anything else. This is the idiom the
                agent row's active-elsewhere menu already uses.

                Reachable at all only because `ui.always_show_tab_strip` renders
                the strip for a single-tab agent; without it there is no pill to
                open a menu on. */}
            {soleTab ? (
              // The group is required, not decorative: a menu label outside one
              // throws in base-ui.
              <DropdownMenuGroup>
                <DropdownMenuLabel className="max-w-60 whitespace-normal">
                  {ONLY_TAB_CLOSE_REFUSAL}
                </DropdownMenuLabel>
                <DropdownMenuItem disabled>
                  <X />
                  Close tab…
                </DropdownMenuItem>
              </DropdownMenuGroup>
            ) : (
              <DropdownMenuItem onClick={() => openCloseTab(session.id, tab.id)}>
                <X />
                Close tab…
              </DropdownMenuItem>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </div>
  )
}
