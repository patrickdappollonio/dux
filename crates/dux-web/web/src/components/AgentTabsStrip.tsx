import {
  Bot,
  Check,
  ChevronDown,
  CircleStop,
  Pin,
  Plus,
  Replace,
  X,
} from "lucide-react"

import { ExplainedWhenDisabled } from "@/components/ExplainedWhenDisabled"
import { MenuItemReason, MenuReasonsHead } from "@/components/MenuReason"
import { useMenuReason } from "@/hooks/use-menu-reason"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { SortableStrip } from "@/components/SortableStrip"
import {
  TAB_STRIP_BUTTON_DISABLED_TAP_AREA,
  TabPill,
  TabStrip,
  TabStripButton,
} from "@/components/TabStrip"
import { AttentionDot } from "@/components/AttentionDot"
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
import {
  ONLY_TAB_CLOSE_REFUSAL,
  defaultProviderForSession,
  isFirstTab,
  sessionAsShown,
  tabLabels,
} from "@/lib/agentTabs"
import {
  addToAgentBlockHint,
  agentChangeBlockReason,
  useRowDriverLabel,
  useAgentChangeBlock,
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
import type { RowBlock } from "@/lib/paneCover"
import { WorkingGlyph } from "@/components/WorkingGlyph"
import type { AgentTabView, SessionView } from "@/lib/types"

const FIRST_TAB_HINT =
  "The agent's first tab: the one that starts when you open a stopped agent."

// The provider-tab strip at the top of the center pane; whether it shows is the
// caller's decision. All tabs render uniformly: clicking a
// pill focuses it, the hover-revealed ⋯ retargets the provider and closes the
// tab, and the trailing + adds one, disabled at the cap or mid-create. While
// this device does not drive the agent, the strip stays above the take-over
// card and everything in it that changes the agent is disabled, as in the
// agent's menu, dragging a pill to reorder included; selecting a tab is not a
// change and stays. A pill
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
  const block = useAgentChangeBlock(session)
  const device = useRowDriverLabel({ sessionId: session.id })
  const disabled = atCap || creating || block !== null
  // Said on the + and on the chooser beside it, whichever reason disables them.
  const newTabHint =
    block !== null
      ? addToAgentBlockHint(block, device)
      : atCap
        ? `Tab limit reached (${cap})`
        : creating
          ? "Adding…"
          : `New ${defaultProvider} tab`

  return (
    <TabStrip
      label="Agent tabs"
      besidePaneDivider
      stripRef={attachStrip}
      trailing={
        <AddTabControls
          disabled={disabled}
          newTabHint={newTabHint}
          providers={providers}
          defaultProvider={defaultProvider}
          sessionId={session.id}
        />
      }
    >
      <SortableStrip
        ids={tabs.map((t) => t.id)}
        onReorder={(next) => reorderTabs(session.id, next)}
        disabled={block !== null}
      >
        {tabs.map((tab, i) => (
          <AgentTabPill
            key={tab.id}
            session={session}
            tab={tab}
            label={labels[i]}
            active={tab.id === activeTabId}
            providers={providers}
            pinned={tabs.length >= 2 && isFirstTab(shown, tab.id)}
            block={block}
          />
        ))}
      </SortableStrip>
    </TabStrip>
  )
}

function AddTabControls({
  disabled,
  newTabHint,
  providers,
  defaultProvider,
  sessionId,
}: {
  disabled: boolean
  newTabHint: string
  providers: string[]
  defaultProvider: string
  sessionId: string
}) {
  return (
    <>
      {/* Split "+" control: the main button quick-adds the project default
          provider, the adjacent caret opens a menu to pick another configured
          one. The gap between the halves is the misclick-safe spacing. On a
          tablet both keep their 24px boxes and take a 40px tap area (8px a
          side), so 16px between them keeps the two areas from overlapping,
          and 2px more before the "+" keeps its area off the last pill's ⋯,
          whose own reaches 1px past its pill. */}
      <div className="flex shrink-0 items-center gap-0.5 md:pointer-coarse:ml-0.5 md:pointer-coarse:gap-4">
        <ExplainedWhenDisabled
          disabled={disabled}
          hint={newTabHint}
          disabledClassName={TAB_STRIP_BUTTON_DISABLED_TAP_AREA}
        >
          <TabStripButton
            aria-label="New tab"
            disabled={disabled}
            onClick={() => addTab(sessionId)}
          >
            <Plus className="size-4" />
          </TabStripButton>
        </ExplainedWhenDisabled>
        <ExplainedWhenDisabled
          disabled={disabled}
          hint={disabled ? newTabHint : null}
          disabledClassName={TAB_STRIP_BUTTON_DISABLED_TAP_AREA}
        >
          <DropdownMenu>
            <DropdownMenuTrigger
              render={
                <TabStripButton aria-label="Choose provider for new tab" disabled={disabled} />
              }
            >
              <ChevronDown className="size-3.5" />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              {providers.map((p) => {
                const isDefault = p === defaultProvider
                return (
                  <DropdownMenuItem key={p} onClick={() => addTab(sessionId, p)}>
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
        </ExplainedWhenDisabled>
      </div>
    </>
  )
}

function AgentTabPill({
  session,
  tab,
  label,
  active,
  providers,
  pinned,
  block,
}: {
  session: SessionView
  tab: AgentTabView
  label: string
  active: boolean
  providers: string[]
  /** This is the agent's first tab and the agent has two or more. */
  pinned: boolean
  /** Why nothing here may change the agent, or null when it may. */
  block: RowBlock | null
}) {
  const changesBlocked = block !== null
  const device = useRowDriverLabel({ sessionId: session.id })
  const changeReason = useMenuReason(
    block === null ? null : agentChangeBlockReason(block, device),
  )
  const sort = usePillSort(tab.id)
  // No successor means no slot to hand on, which is the one close the server
  // refuses. Answered from the tab list rather than from slot-ness, because
  // "this is the first tab" and "this is the only tab" are different facts and
  // only the second one refuses.
  const soleTab = session.tabs.length <= 1
  const soleTabReason = useMenuReason(soleTab ? ONLY_TAB_CLOSE_REFUSAL : null)

  function select() {
    clickTab(session.id, tab.id)
  }

  return (
    <TabPill
      data-cue-key={tab.id}
      active={active}
      sort={sort}
      onSelect={select}
      // The same glyph cue as the sidebar, on the pill's own ladder: typing and
      // needs-you outrank working, so either one stands the cue down.
      icon={
        <WorkingGlyph
          icon={Bot}
          working={tab.working && !tab.typing && !tab.needs_attention}
          className="size-3.5"
        />
      }
      // A permission prompt or a finished turn on this specific tab.
      badge={tab.needs_attention ? <AttentionDot /> : null}
      label={label}
      // The pin marks the tab that starts and resumes the agent, in the quiet
      // text color. It follows the slot when the slot passes on.
      marker={
        pinned ? (
          <SimpleTooltip content={FIRST_TAB_HINT}>
            <span role="img" aria-label={FIRST_TAB_HINT} className="shrink-0 text-muted-foreground">
              <Pin className="size-3" aria-hidden />
            </span>
          </SimpleTooltip>
        ) : null
      }
      menu={{
        label: "Tab actions",
        content: (
          <DropdownMenuContent align="start" onClick={(e) => e.stopPropagation()}>
            <MenuReasonsHead reasons={[changeReason]} />
            <DropdownMenuSub>
              <DropdownMenuSubTrigger
                disabled={changesBlocked}
                aria-describedby={changeReason.describedBy}
              >
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
                disabled={changesBlocked}
                aria-describedby={changeReason.describedBy}
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
                said beneath it: a disabled item is `pointer-events-none`, so a
                tooltip hung on it could never be opened, by a pointer or by
                anything else.

                Reachable at all only because `ui.always_show_tab_strip` renders
                the strip for a single-tab agent; without it there is no pill to
                open a menu on. */}
            {soleTab ? (
              <>
                <DropdownMenuItem disabled aria-describedby={soleTabReason.describedBy}>
                  <X />
                  Close tab…
                </DropdownMenuItem>
                <MenuItemReason reason={soleTabReason} />
              </>
            ) : (
              <DropdownMenuItem
                disabled={changesBlocked}
                aria-describedby={changeReason.describedBy}
                onClick={() => openCloseTab(session.id, tab.id)}
              >
                <X />
                Close tab…
              </DropdownMenuItem>
            )}
          </DropdownMenuContent>
        ),
      }}
    />
  )
}
