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
import type { TerminalOwnerRef } from "@/lib/store"
import type { SessionView } from "@/lib/types"

/// The names the control answers to, one per kind of pane. They are the names
/// the sidebar rows already use, so the row and the pane header cannot end up
/// calling one menu two things, and a screen reader or a voice command does not
/// have to learn which of the four anchors is on screen.
export const PANE_MENU_AGENT_LABEL = "Session actions"
export const PANE_MENU_TERMINAL_LABEL = "Terminal actions"

/// WHAT THE PANE IS ABOUT, which is the only thing that changes the menu's
/// contents. An agent's menu is its actions; a terminal's is its own. Everything
/// around them (the INPUT group above, the theater exit, the Settings drill) is
/// the same for both, because those are about the SURFACE rather than about
/// what is behind it.
export type PaneMenuSubject =
  | { kind: "agent"; session: SessionView }
  | { kind: "terminal"; terminalId: string; owner: TerminalOwnerRef }

/// How the trigger is drawn, which is the only OTHER thing that varies between
/// the surfaces that anchor this menu.
///
/// `cluster` is the bare 40px circle the flap and the pill are built from: both
/// are ONE rounded surface, and a bordered button inside either reads as two.
/// `header` is the desktop pane header's outline treatment, shared with the
/// Macros trigger and the theater button it sits beside.
export type PaneMenuAppearance = "cluster" | "header"

function paneMenuLabel(subject: PaneMenuSubject): string {
  return subject.kind === "agent"
    ? PANE_MENU_AGENT_LABEL
    : PANE_MENU_TERMINAL_LABEL
}

// THE PANE'S ONE MENU, wherever a surface anchors it.
//
// Four anchors, one body per kind of pane: the phone's docked flap, the
// floating pill, the desktop pane header's `⋯`, and the thing's own row in the
// sidebar. They used to answer to different menus, which made theater on a
// phone the one state in which renaming, deleting, the tab and project submenus
// and every other per-entity action were simply unreachable, and it made the
// flight a lie: the cluster flies across the screen as one object precisely
// because it IS one object, and one of its buttons changing what it does on
// arrival is the thing the animation says cannot happen. The desktop header's
// `⋯` joins them for the same reason in the other direction: it is the pane's
// top menu, so it is the whole menu rather than a subset somebody has to
// remember the shape of.
//
// The body is, in this order: the pane's INPUT group, then the subject's own
// actions, then the way out of theater while the mode is on, then the app menu
// as a drill named for the cog it stands in for. A phone renders every one of
// these menus as a bottom sheet with its own stacked sub-sheets, so a submenu
// is the sheet's own idiom rather than a flyout squeezed against an edge.
//
// THE INPUT GROUP IS FIRST, above the subject's actions, because this menu is
// the only permanent home the virtual input's controls have: typing directly in
// the terminal takes the whole bottom bar away, and the way back has to be
// somewhere that never leaves. It is a thumb's reach from the `⋯` that opened
// the sheet, which is where the hand already is.
//
// The anchors differ in exactly two things, neither of them content: where the
// menu opens from, and what the trigger looks like. The INPUT group's ITEMS are
// still one home at a time, because what the group contains is the pane's own
// published answer (see `lib/paneInputGroup.ts`) rather than a per-anchor
// decision: two anchors of this one menu show the same rows, and the split that
// matters is between this top menu and the input `⋯` down in the bottom bar.
//
// There is no "Changes ±N" row: an agent's flap and pill both carry a real
// count BUTTON beside this trigger, keyboard-reachable and labelled for a
// screen reader, and a second copy of it in the menu was two places for the
// same number to be printed. A terminal has no count at all.
export function PaneMenu({
  subject,
  side = "bottom",
  appearance = "cluster",
}: {
  subject: PaneMenuSubject
  /// Which way the menu opens on a surface wide enough to anchor it: the flap
  /// hangs from the band at the top of the pane, the pill floats at the bottom.
  /// A phone ignores it and renders a sheet.
  side?: "top" | "bottom"
  appearance?: PaneMenuAppearance
}) {
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
        <PaneMenuBody subject={subject} />
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

/// Everything a pane's menu carries, ready to drop into any content.
export function PaneMenuBody({ subject }: { subject: PaneMenuSubject }) {
  const theater = useDux().theater
  // The ptys this surface could be about, and the subject's own rows. An agent
  // answers for its session-slot id and every tab id, because any one of its
  // panes can be the mounted one; a terminal is one pty and answers for itself.
  let ptyIds: string[]
  let actions: ReactNode
  if (subject.kind === "agent") {
    const { session } = subject
    ptyIds = [session.id, ...session.tabs.map((t) => t.id)]
    actions = <AgentActionsMenu session={session} />
  } else {
    ptyIds = [subject.terminalId]
    actions = (
      <TerminalActionsMenu
        terminalId={subject.terminalId}
        owner={subject.owner}
      />
    )
  }
  return (
    <>
      {/* Whichever pane is mounted and owns its input answers for the whole
          group, exactly as the row menus borrow the attach: an upload travels
          through that pane's own gated connection and lands in its own sink,
          and the surface items are that pane's own state. */}
      <PaneInputGroup ptyIds={ptyIds} />
      {actions}
      {theater ? (
        <>
          <DropdownMenuSeparator />
          {/* The way back, from the surface the mode leaves on screen. */}
          <InputMenuItems theaterExit />
        </>
      ) : null}
      <DropdownMenuSeparator />
      {/* NAMED FOR THE CONTROL IT STANDS IN FOR. Theater takes the top bar,
          and with it the cog; on a phone the flap's own header is gone in both
          modes. A user looking for the app's own actions should find them under
          the name they know, and in theater this is the one trigger on screen.
          It rides along at every anchor rather than being conditioned on the
          mode: one body, so the menu a user learns in one place is the menu
          they get in the others. */}
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
  )
}
