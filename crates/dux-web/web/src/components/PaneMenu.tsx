import { Ellipsis, Settings } from "lucide-react"
import type { ReactNode } from "react"

import { AppMenuBody } from "@/components/AppMenu"
import { AgentActionsMenu } from "@/components/AgentActionsMenu"
import { InputMenuItems } from "@/components/InputMenuItems"
import { PaneInputGroup } from "@/components/PaneInputGroup"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { TerminalActionsMenu } from "@/components/TerminalActionsMenu"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { useDux } from "@/lib/store"
import type { SelectedTarget, TerminalOwnerRef } from "@/lib/store"
import type { SessionView } from "@/lib/types"

/// The names the control answers to, one per kind of pane, matching the sidebar
/// rows so no anchor calls one menu something different.
export const PANE_MENU_AGENT_LABEL = "Session actions"
export const PANE_MENU_TERMINAL_LABEL = "Terminal actions"

/// What the terminal-scoped group is called inside an agent's menu, when the
/// pane on screen is one of that agent's companion terminals.
export const PANE_MENU_TERMINAL_GROUP_LABEL = "Terminal"

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
// varies between anchors is placement, the trigger, and that drill, never a row.
// There is deliberately no count row: an agent's flap and pill carry a real one.
export function PaneMenu({
  subject,
  pane,
  side = "bottom",
  appearance = "cluster",
  settingsDrill,
}: {
  subject: PaneMenuSubject
  /// The pane this anchor is painted over, for the anchors that are on one.
  pane?: PaneMenuPane
  /// Which way the menu opens on a surface wide enough to anchor it. A phone
  /// ignores it and renders a sheet.
  side?: "top" | "bottom"
  appearance?: PaneMenuAppearance
} & PaneMenuSettingsDrill) {
  const cluster = appearance === "cluster"
  const label = paneMenuLabel(subject)
  return (
    <DropdownMenu>
      <SimpleTooltip content={label}>
        <DropdownMenuTrigger
          render={
            <Button
              variant={cluster ? "ghost" : "outline"}
              size="icon"
              className={cluster ? "size-10 shrink-0 rounded-full" : "shrink-0"}
              aria-label={label}
            />
          }
        >
          <Ellipsis />
        </DropdownMenuTrigger>
      </SimpleTooltip>
      <DropdownMenuContent align="end" side={side}>
        <PaneMenuBody
          subject={subject}
          pane={pane}
          settingsDrill={settingsDrill}
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
}: {
  subject: PaneMenuSubject
  pane?: PaneMenuPane
} & PaneMenuSettingsDrill) {
  const theater = useDux().theater
  // The subject's own rows.
  const actions: ReactNode =
    subject.kind === "agent" ? (
      <AgentActionsMenu session={subject.session} />
    ) : (
      <TerminalActionsMenu
        terminalId={subject.terminalId}
        owner={subject.owner}
      />
    )
  // The pane's own verbs, when a companion terminal sits under its agent's menu.
  // Labelled and last, so the agent body keeps its order and Close… reads as the
  // terminal's rather than the agent's.
  const paneActions: ReactNode =
    subject.kind === "agent" && pane?.kind === "terminal" ? (
      <>
        <DropdownMenuSeparator />
        <TerminalActionsMenu
          terminalId={pane.terminalId}
          owner={pane.owner}
          label={PANE_MENU_TERMINAL_GROUP_LABEL}
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
      {actions}
      {paneActions}
      {theater ? (
        <>
          <DropdownMenuSeparator />
          {/* The way back, from the surface the mode leaves on screen. */}
          <InputMenuItems theaterExit />
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
          <DropdownMenuSub>
            <DropdownMenuSubTrigger>
              <Settings />
              Settings
            </DropdownMenuSubTrigger>
            <DropdownMenuSubContent side="left">
              <AppMenuBody />
            </DropdownMenuSubContent>
          </DropdownMenuSub>
        </>
      ) : null}
    </>
  )
}
