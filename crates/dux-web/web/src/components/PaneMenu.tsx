import { Maximize2, Plus, Settings, SquareSlash } from "lucide-react"
import { type ReactNode, type Ref } from "react"

import { EllipsisMenuTrigger } from "@/components/EllipsisMenuTrigger"
import { AppMenuBody } from "@/components/AppMenu"
import { AgentActionsMenu } from "@/components/AgentActionsMenu"
import { useAgentChangeReason } from "@/hooks/use-agent-change-reason"
import { InputMenuItems } from "@/components/InputMenuItems"
import { MenuItemReason } from "@/components/MenuReason"
import { useMenuReason } from "@/hooks/use-menu-reason"
import { PaneInputGroup } from "@/components/PaneInputGroup"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { TerminalActionsMenu } from "@/components/TerminalActionsMenu"
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
} from "@/components/ui/dropdown-menu"
import { MENU_GROUP_LABELS } from "@/lib/menuGroupLabels"
import { useAgentRowWatch } from "@/lib/paneCover"
import { useDux } from "@/lib/store"
import { spineListsTerminal } from "@/lib/terminals"
import type { SelectedTarget, TerminalOwnerRef } from "@/lib/store"
import type { SessionView } from "@/lib/types"

/// The names the control answers to, one per kind of pane, matching the sidebar
/// rows so no anchor calls one menu something different.
export const PANE_MENU_AGENT_LABEL = "Session actions"
export const PANE_MENU_TERMINAL_LABEL = "Terminal actions"

/// The pty ids the INPUT group is read under. A surface over a pane asks for that
/// pane's pty; a sidebar row falls back to every pty its subject could be mounted as.
function paneInputPtyIds(
  subject: PaneMenuSubject,
  pane: PaneMenuPane | undefined,
): string[] {
  if (pane) return [pane.kind === "agent" ? pane.tabId : pane.terminalId]
  if (subject.kind === "agent") {
    return [subject.session.id, ...subject.session.tabs.map((t) => t.id)]
  }
  return [subject.terminalId]
}

/// What the pane is about, which is the only thing that changes the menu's
/// contents: everything around the subject's actions is about the surface.
export type PaneMenuSubject =
  | { kind: "agent"; session: SessionView }
  | { kind: "terminal"; terminalId: string; owner: TerminalOwnerRef }

/// Which pane this anchor is painted over, asked separately from what the menu is
/// about: a session-owned terminal's pane registers under the TERMINAL's id while
/// the menu over it is the agent's, so the subject's ids would find nothing. A
/// sidebar row is not over a pane and passes none of this.
export type PaneMenuPane = SelectedTarget

/// How the trigger is drawn. `cluster` is the bare 40px circle the flap and pill
/// are built from, since a bordered button inside either reads as two surfaces;
/// `header` is the outline treatment shared with Macros and the theater button.
export type PaneMenuAppearance = "cluster" | "header"

/// Whether the app menu rides along as a "Settings" drill: only where the app-menu
/// cog is NOT on screen, which is the one thing only the anchor knows. No default,
/// so a new anchor answers it rather than inheriting somebody else's chrome.
export type PaneMenuSettingsDrill = {
  settingsDrill: boolean
}

/// The computer header's controls that are folded into its `⋯` now: each row
/// runs what its button runs, and a control still in the header is absent.
/// `newTabBlocked` and `macrosBlocked` are the reasons those buttons would be
/// disabled.
export type FoldedHeaderControls = {
  newTab?: () => void
  newTabBlocked?: string | null
  theater?: () => void
  macros?: () => void
  macrosBlocked?: string | null
}

function paneMenuLabel(subject: PaneMenuSubject): string {
  return subject.kind === "agent"
    ? PANE_MENU_AGENT_LABEL
    : PANE_MENU_TERMINAL_LABEL
}

// The pane's one menu, wherever a surface anchors it: one body per kind of pane,
// in this order. The pane's INPUT group first, because this menu is the only
// permanent home the virtual input's controls have; then the subject's actions,
// the pane's own verbs when the pane is not the subject, the theater exit, and
// the Settings drill where the anchor stands somewhere the cog does not. What
// varies between anchors is placement, the trigger, that drill, and the computer
// header's own controls while it has folded them in, never a row of the pane's.
// There is deliberately no count row: an agent's flap and pill carry a real one.
export function PaneMenu({
  subject,
  pane,
  side = "bottom",
  appearance = "cluster",
  settingsDrill,
  folded,
  triggerRef,
  onCloseComplete,
}: {
  subject: PaneMenuSubject
  /// The pane this anchor is painted over, for the anchors that are on one.
  pane?: PaneMenuPane
  /// Which way the menu opens on a surface wide enough to anchor it. A phone
  /// ignores it and renders a sheet.
  side?: "top" | "bottom"
  appearance?: PaneMenuAppearance
  /// The header's controls, while the header has folded them in here.
  folded?: FoldedHeaderControls
  triggerRef?: Ref<HTMLButtonElement>
  /// Called once the menu has finished closing, for a row whose dialog or
  /// picker should open only after the menu has handed focus back.
  onCloseComplete?: () => void
} & PaneMenuSettingsDrill) {
  const cluster = appearance === "cluster"
  const label = paneMenuLabel(subject)
  return (
    <DropdownMenu
      onOpenChangeComplete={(open) => {
        if (!open) onCloseComplete?.()
      }}
    >
      <SimpleTooltip content={label}>
        <EllipsisMenuTrigger
          ref={triggerRef}
          placement={cluster ? "floating" : "header"}
          aria-label={label}
        />
      </SimpleTooltip>
      <DropdownMenuContent align="end" side={side}>
        <PaneMenuBody
          subject={subject}
          pane={pane}
          settingsDrill={settingsDrill}
          folded={folded}
        />
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

/// Everything a pane's menu carries, ready to drop into any content.
export function PaneMenuBody({
  subject,
  pane,
  settingsDrill,
  folded,
}: {
  subject: PaneMenuSubject
  pane?: PaneMenuPane
  folded?: FoldedHeaderControls
} & PaneMenuSettingsDrill) {
  const { theater, spine } = useDux()
  // Under the agent's take-over card nothing names one of its terminals.
  const watched = useAgentRowWatch(
    subject.kind === "agent" ? subject.session.id : null,
  ).watched
  // The agent's one reason, made here so the terminal group beneath its
  // actions can name the line the agent's menu says at its head.
  const agentReason = useAgentChangeReason(subject.kind === "agent" ? subject.session : null)
  // The subject's own rows.
  const actions: ReactNode =
    subject.kind === "agent" ? (
      <AgentActionsMenu session={subject.session} changeReason={agentReason} />
    ) : (
      <TerminalActionsMenu
        terminalId={subject.terminalId}
        owner={subject.owner}
      />
    )
  // The pane's own verbs, when a companion terminal sits under its agent's menu.
  // Labelled and last, so the agent body keeps its order and Close… reads as the
  // terminal's rather than the agent's.
  // Its editor entries are the agent's, so until the list names the terminal
  // (and its Close… has something to confirm) the group would be empty.
  const paneActions: ReactNode =
    subject.kind === "agent" &&
    pane?.kind === "terminal" &&
    !watched &&
    spineListsTerminal(spine?.terminals, pane.terminalId) ? (
      <>
        <DropdownMenuSeparator />
        <TerminalActionsMenu
          terminalId={pane.terminalId}
          owner={pane.owner}
          // The agent's own group above already opens this same editor.
          editorEntries={false}
          // And already says why the agent's row may not change.
          reasonAbove={agentReason}
        />
      </>
    ) : null
  return (
    <>
      {/* Whichever pane is mounted and owns its input answers for the whole
          group, exactly as the row menus borrow the attach: an upload travels
          through that pane's own gated connection and lands in its own sink,
          and the surface items are that pane's own state. */}
      <PaneInputGroup ptyIds={paneInputPtyIds(subject, pane)} />
      {folded ? <FoldedHeaderGroup controls={folded} theater={theater} /> : null}
      {actions}
      {paneActions}
      {theater ? (
        <>
          <DropdownMenuSeparator />
          {/* The way back, from the surface the mode leaves on screen. */}
          <DropdownMenuGroup>
            <DropdownMenuLabel>{MENU_GROUP_LABELS.theater}</DropdownMenuLabel>
            <InputMenuItems theaterExit />
          </DropdownMenuGroup>
        </>
      ) : null}
      {settingsDrill ? (
        <>
          <DropdownMenuSeparator />
          {/* NAMED FOR THE CONTROL IT STANDS IN FOR, and rendered only where
              that control is not. Theater takes the top bar and with it the
              cog, and a phone pane screen's header never had one; a user
              looking for the app's own actions should find them under the name
              they know. Where the cog IS on screen (the desktop header's
              top-right corner, the phone hub's own header) this is the same
              body offered twice, so the anchor says no. See
              `PaneMenuSettingsDrill`. */}
          <DropdownMenuGroup>
            <DropdownMenuLabel>{MENU_GROUP_LABELS.app}</DropdownMenuLabel>
            <DropdownMenuSub>
              <DropdownMenuSubTrigger>
                <Settings />
                Settings
              </DropdownMenuSubTrigger>
              <DropdownMenuSubContent side="left">
                <AppMenuBody />
              </DropdownMenuSubContent>
            </DropdownMenuSub>
          </DropdownMenuGroup>
        </>
      ) : null}
    </>
  )
}

// The computer header's controls, while it has folded them in here to give the
// names their room. Right after the Input group, which keeps the menu's head:
// they were the first controls on the header, so the top of the menu is where a
// person who saw them leave looks first. Theater keeps its word, because the
// mode has no other way in; while it is on, the theater group's exit is the one
// row for it.
function FoldedHeaderGroup({
  controls,
  theater,
}: {
  controls: FoldedHeaderControls
  theater: boolean
}) {
  const macrosReason = useMenuReason(controls.macrosBlocked ?? null)
  const newTabReason = useMenuReason(controls.newTabBlocked ?? null)
  const theaterRow = controls.theater && !theater ? controls.theater : null
  // Theater folded alone while the mode is on leaves nothing to head.
  if (!controls.newTab && !theaterRow && !controls.macros) return null
  return (
    <>
      <DropdownMenuGroup>
        <DropdownMenuLabel>{MENU_GROUP_LABELS.pane}</DropdownMenuLabel>
        {controls.newTab ? (
          <>
            <DropdownMenuItem
              disabled={newTabReason.text !== null}
              aria-describedby={newTabReason.describedBy}
              onClick={controls.newTab}
            >
              <Plus />
              New tab…
            </DropdownMenuItem>
            <MenuItemReason reason={newTabReason} />
          </>
        ) : null}
        {theaterRow ? (
          <DropdownMenuItem onClick={theaterRow}>
            <Maximize2 />
            Theater
          </DropdownMenuItem>
        ) : null}
        {/* The reason the Macros… button's tooltip gives, said beneath the row. */}
        {controls.macros ? (
          <>
            <DropdownMenuItem
              disabled={macrosReason.text !== null}
              aria-describedby={macrosReason.describedBy}
              onClick={controls.macros}
            >
              <SquareSlash />
              Macros…
            </DropdownMenuItem>
            <MenuItemReason reason={macrosReason} />
          </>
        ) : null}
      </DropdownMenuGroup>
      <DropdownMenuSeparator />
    </>
  )
}
