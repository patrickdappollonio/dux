import {
  Bot,
  Check,
  ChevronDown,
  CircleStop,
  Ellipsis,
  Pin,
  Plus,
  Replace,
  X,
} from "lucide-react"

import { SimpleTooltip } from "@/components/SimpleTooltip"
import { SortableStrip } from "@/components/SortableStrip"
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
  isFirstTab,
  sessionAsShown,
  tabLabels,
} from "@/lib/agentTabs"
import {
  ADD_TO_AGENT_ELSEWHERE_HINT,
  AGENT_ELSEWHERE_REASON,
  useAgentActiveElsewhere,
} from "@/hooks/use-agent-active-elsewhere"
import { useKeepCueClocks } from "@/hooks/use-keep-cue-clocks"
import { usePillSort } from "@/hooks/use-pill-sort"
import {
  addTab,
  clickTab,
  openCloseTab,
  openStopTab,
  reorderTabs,
  retargetTab,
  useDux,
} from "@/lib/store"
import { DEFAULT_AGENT_TABS_MAX } from "@/lib/bootstrapApi"
import { HOVER_REVEAL, REVEAL_ROW, REVEALED_ON_ANY_FOCUS } from "@/lib/hoverReveal"
import { focusAdjacentTab, scrollFocusedTabIntoView } from "@/lib/tabFocusScroll"
import { cn } from "@/lib/utils"
import { WorkingGlyph } from "@/components/WorkingGlyph"
import type { AgentTabView, SessionView } from "@/lib/types"

const FIRST_TAB_HINT =
  "The agent's first tab: the one that starts when you open a stopped agent."

// The provider-tab strip at the top of the center pane; whether it shows is the
// caller's decision. All tabs render uniformly: clicking a
// pill focuses it, the hover-revealed ⋯ retargets the provider and closes the
// tab, and the trailing + adds one, disabled at the cap or mid-create. While
// another device drives the agent, the strip stays above the take-over card and
// everything in it that changes the agent is disabled, as in the agent's menu;
// selecting a tab is not a change and stays. A pill
// drags along the strip to reorder it, like a browser tab; the order is the
// agent's own and survives a restart. The agent's first tab, the one that starts
// and resumes the agent, wears a pin while there are two or more tabs.
export function AgentTabsStrip({
  session,
  activeTabId,
  maxTabs,
}: {
  session: SessionView
  activeTabId: string
  maxTabs?: number
}) {
  const { bootstrap, spine, createTabInFlight, pendingTabOrder, pendingSlotTab } =
    useDux()
  const providers = bootstrap?.available_providers ?? []
  // The order just dragged shows at once; the server's answer replaces it.
  const shown = sessionAsShown(
    session,
    pendingTabOrder[session.id],
    pendingSlotTab[session.id],
  )
  const tabs = shown.tabs
  const labels = tabLabels(tabs)
  const attachStrip = useKeepCueClocks(tabs.map((t) => t.id).join("\n"))
  const cap = maxTabs ?? DEFAULT_AGENT_TABS_MAX
  const atCap = tabs.length >= cap
  const creating = createTabInFlight.includes(session.id)
  const defaultProvider = defaultProviderForSession(spine, session)
  const elsewhere = useAgentActiveElsewhere(session)
  const disabled = atCap || creating || elsewhere

  return (
    // `max-md:py-0.75` trims the strip's own vertical padding to go with the
    // shorter phone pill below: shrinking the pill alone would leave the strip
    // the same height. It stays at 3px, the reach of a focus ring, because the
    // scroller clips anything past it.
    //
    // A finger at desktop width (a tablet) gets 8px between pills, so one
    // pill's ⋯ tap margin stays clear of the next pill, and an 18px end: the
    // pane divider's touch band reaches 10px into this strip, and the
    // chooser's tap margin takes the other 8.
    <div
      ref={attachStrip}
      onFocus={(e) => scrollFocusedTabIntoView(e.target)}
      className="flex items-center gap-1 overflow-x-auto border-b bg-muted/30 px-2 py-1.25 scroll-px-2 max-md:py-0.75 md:pointer-coarse:gap-2 md:pointer-coarse:pr-4.5"
    >
      <SortableStrip
        ids={tabs.map((t) => t.id)}
        onReorder={(next) => reorderTabs(session.id, next)}
      >
        {tabs.map((tab, i) => (
          <TabPill
            key={tab.id}
            session={session}
            tab={tab}
            label={labels[i]}
            active={tab.id === activeTabId}
            providers={providers}
            pinned={tabs.length >= 2 && isFirstTab(shown, tab.id)}
            elsewhere={elsewhere}
          />
        ))}
      </SortableStrip>
      {/* Split "+" control: the main button quick-adds the project default
          provider, the adjacent caret opens a menu to pick another configured
          one. The gap between the halves is the misclick-safe spacing. On a
          tablet both keep their 24px boxes and take a 40px tap area (8px a
          side), so 16px between them keeps the two areas from overlapping,
          and 2px more before the "+" keeps its area off the last pill's ⋯,
          whose own reaches 1px past its pill. */}
      <div className="flex shrink-0 items-center gap-0.5 md:pointer-coarse:ml-0.5 md:pointer-coarse:gap-4">
        <SimpleTooltip
          content={
            elsewhere
              ? ADD_TO_AGENT_ELSEWHERE_HINT
              : atCap
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
            // On a tablet the box stays 24px and an invisible margin takes the
            // tap area to 40px.
            className="relative flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors after:absolute after:-inset-2 after:hidden hover:bg-background hover:text-foreground disabled:pointer-events-none disabled:opacity-40 max-md:h-9 max-md:w-11 md:pointer-coarse:after:block"
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
                // Height relaxed, width kept, and the same tablet tap margin:
                // same reasoning as the "+" beside it.
                className="relative flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors after:absolute after:-inset-2 after:hidden hover:bg-background hover:text-foreground disabled:pointer-events-none disabled:opacity-40 max-md:h-9 max-md:w-11 md:pointer-coarse:after:block"
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
  pinned,
  elsewhere,
}: {
  session: SessionView
  tab: AgentTabView
  label: string
  active: boolean
  providers: string[]
  /** This is the agent's first tab and the agent has two or more. */
  pinned: boolean
  /** Another device drives the agent, so nothing here may change it. */
  elsewhere: boolean
}) {
  const { setNodeRef, style, listeners, isDragging } = usePillSort(tab.id)
  // No successor means no slot to hand on, which is the one close the server
  // refuses. Answered from the tab list rather than from slot-ness, because
  // "this is the first tab" and "this is the only tab" are different facts and
  // only the second one refuses.
  const soleTab = session.tabs.length <= 1

  function select() {
    clickTab(session.id, tab.id)
  }

  return (
    <div
      ref={setNodeRef}
      style={style}
      data-cue-key={tab.id}
      {...listeners}
      role="tab"
      aria-selected={active}
      tabIndex={0}
      onClick={select}
      onKeyDown={(e) => {
        // Only a press on the pill itself: Enter and Space from the ⋯ trigger
        // and its menu items bubble here too (through the menu's portal), and
        // must not select the tab or start one resting on the idle screen.
        if (e.target !== e.currentTarget) return
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault()
          select()
        }
        focusAdjacentTab(e)
      }}
      // `max-md:min-h-9` (36px) relaxes the 40px touch-target floor on the
      // vertical axis only. The neighbours there are the mobile header, whose
      // controls end at its bottom edge, and the PTY, where a mis-tap costs a
      // keyboard you dismiss or a click the app ignores. Horizontally the pill
      // keeps its size, because its neighbours are other tabs and landing on
      // the wrong one is a real mis-tap. A tablet gets the same 36px for the
      // same reasons, with the strip's own padding kept, so the ⋯ tap area
      // inside, which overshoots the pill by 2px, stays inside the strip.
      className={cn(
        // `max-md:py-0` goes with it: the pill's own 4px padding would sit on
        // top of the 32px ⋯ hit area inside and overshoot the 36px again.
        "flex shrink-0 cursor-pointer touch-manipulation select-none [-webkit-touch-callout:none] items-center gap-1.5 rounded-md border px-2 py-1 text-sm transition-colors max-md:min-h-9 max-md:py-0 md:pointer-coarse:min-h-9",
        REVEAL_ROW,
        isDragging && "relative z-10 shadow-lg",
        active
          ? "border-border bg-background text-foreground"
          : "border-transparent bg-muted text-muted-foreground hover:text-foreground",
      )}
    >
      {/* The same glyph cue as the sidebar, on the pill's own ladder: typing
          and needs-you outrank working, so either one stands the cue down.
          Every stop finishes the bounce at rest and eases the glyph back to
          full; the attention dot is a separate element beside it, so it
          appears at once. */}
      <WorkingGlyph
        icon={Bot}
        working={tab.working && !tab.typing && !tab.needs_attention}
        className="size-3.5"
      />
      {/* Cyan attention dot on the flagged tab's pill (a permission prompt or a
          finished turn on this specific tab). */}
      {tab.needs_attention && <AttentionDot />}
      <span className="max-w-40 truncate">{label}</span>
      {/* The pin marks the tab that starts and resumes the agent, in the quiet
          text color. It follows the slot when the slot passes on. */}
      {pinned ? (
        <SimpleTooltip content={FIRST_TAB_HINT}>
          <span role="img" aria-label={FIRST_TAB_HINT} className="shrink-0 text-muted-foreground">
            <Pin className="size-3" aria-hidden />
          </span>
        </SimpleTooltip>
      ) : null}
      {/* The shared reveal, also on mouse focus in the pill. The phone keeps
          the slot's width open for its 44px trigger. */}
      <div
        className={cn(
          HOVER_REVEAL,
          "[--reveal-cap:--spacing(8)] max-md:max-w-none",
          REVEALED_ON_ANY_FOCUS,
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
                // the pill back to 44px and undone the shorter strip. A tablet
                // keeps the 20px box and an invisible 10px margin takes the tap
                // area to 40px. Its left 10px lie over the pill's own label or
                // pin, so a tap there opens the menu rather than selecting the
                // tab.
                className="relative flex size-5 shrink-0 items-center justify-center rounded text-muted-foreground after:absolute after:-inset-2.5 after:hidden hover:text-foreground max-md:h-8 max-md:w-11 md:pointer-coarse:after:block"
              />
            }
          >
            <Ellipsis className="size-3.5" />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" onClick={(e) => e.stopPropagation()}>
            {elsewhere ? (
              <DropdownMenuGroup>
                <DropdownMenuLabel className="max-w-60 whitespace-normal">
                  {AGENT_ELSEWHERE_REASON}
                </DropdownMenuLabel>
                <DropdownMenuSeparator />
              </DropdownMenuGroup>
            ) : null}
            <DropdownMenuSub>
              <DropdownMenuSubTrigger disabled={elsewhere}>
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
            {/* Unlike Close, the only tab can stop too, since the tab stays.
                Absent rather than disabled with no process, like Detach. */}
            {tab.has_live_process ? (
              <DropdownMenuItem
                disabled={elsewhere}
                onClick={() => openStopTab(session.id, tab.id)}
              >
                <CircleStop />
                Stop tab…
              </DropdownMenuItem>
            ) : null}
            {/* Every tab closes the same way, the agent's FIRST included: the
                slot is a pointer, so closing the tab holding it hands the slot
                to the leftmost other tab rather than being refused. The
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
              <DropdownMenuItem
                disabled={elsewhere}
                onClick={() => openCloseTab(session.id, tab.id)}
              >
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
