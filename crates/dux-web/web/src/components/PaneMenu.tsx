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

/// The names the control answers to, one per kind of pane. They are the names
/// the sidebar rows already use, so the row and the pane header cannot end up
/// calling one menu two things, and a screen reader or a voice command does not
/// have to learn which of the four anchors is on screen.
export const PANE_MENU_AGENT_LABEL = "Session actions"
export const PANE_MENU_TERMINAL_LABEL = "Terminal actions"

/// What the terminal-scoped group is called inside an agent's menu, when the
/// pane on screen is one of that agent's companion terminals.
export const PANE_MENU_TERMINAL_GROUP_LABEL = "Terminal"

/// THE PTY IDS THE INPUT GROUP IS READ UNDER, which is the PANE's question and
/// not the subject's.
///
/// A surface painted over a pane knows exactly which pty it is looking at, so it
/// asks for that one. A sidebar row has no pane, so it falls back to every pty
/// its subject could be mounted as: an agent answers for its session-slot id and
/// every tab id, because any one of its panes can be the mounted one, and a
/// terminal is one pty and answers for itself.
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

/// WHAT THE PANE IS ABOUT, which is the only thing that changes the menu's
/// contents. An agent's menu is its actions; a terminal's is its own. Everything
/// around them (the INPUT group above, the theater exit, the Settings drill
/// where the anchor asks for one) is the same for both, because those are about
/// the SURFACE rather than about what is behind it.
export type PaneMenuSubject =
  | { kind: "agent"; session: SessionView }
  | { kind: "terminal"; terminalId: string; owner: TerminalOwnerRef }

/// WHICH PANE THIS ANCHOR IS PAINTED OVER, which is a different question from
/// what the menu is about and has to be asked separately. The pane the anchor
/// is on does not decide the Settings drill; the chrome AROUND the anchor does,
/// which is a separate answer again (see `PaneMenuSettingsDrill`).
///
/// The INPUT group is the pane's own published answer, keyed by the pty id the
/// pane registers under, and a session-owned terminal's pane registers under the
/// TERMINAL's id while the menu above it is the agent's. Deriving the ids from
/// the subject therefore looked in the agent's ids and found nothing, which left
/// that pane with no "Attach a file…" and, once the user had asked to type
/// straight into the terminal, no way back at all.
///
/// A sidebar row passes none of this: it is a row in a list rather than a
/// surface over a pane, so the subject's own ids are the only sensible scan and
/// the fallback below is what it gets.
export type PaneMenuPane = SelectedTarget

/// How the trigger is drawn, which is the only OTHER thing that varies between
/// the surfaces that anchor this menu.
///
/// `cluster` is the bare 40px circle the flap and the pill are built from: both
/// are ONE rounded surface, and a bordered button inside either reads as two.
/// `header` is the desktop pane header's outline treatment, shared with the
/// Macros trigger and the theater button it sits beside.
export type PaneMenuAppearance = "cluster" | "header"

/// WHETHER THE APP MENU RIDES ALONG AS A "Settings" DRILL, which is the one
/// thing about this body an anchor decides, because it is the one thing only
/// the anchor knows: what chrome is on screen around it.
///
/// THE SETTINGS DRILL RENDERS ONLY WHERE THE APP-MENU COG IS NOT ON SCREEN. The
/// drill exists so the app's own actions are never unreachable, not so they are
/// reachable twice: a computer keeps the cog in the header's top-right corner
/// for as long as that header is mounted, so a second copy of the same body two
/// controls to its left is duplication, and the phone's hub header carries its
/// own cog above the very rows whose menus would repeat it. The anchors that
/// keep it are the ones whose surface has no cog at all: a phone pane screen,
/// whose header is Back and identity only, and the floating pill, which is what
/// theater leaves on screen after unmounting the chrome the cog lives in.
///
/// It is the same idiom as the anchor-decides-placement rules above: the body is
/// one body, and the anchor passes what only it can answer. It has no default,
/// so a new anchor has to answer it rather than inherit somebody else's chrome.
export type PaneMenuSettingsDrill = {
  settingsDrill: boolean
}

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
// actions, then the pane's own verbs when the pane is not the subject, then the
// way out of theater while the mode is on, and last, only where the anchor
// stands somewhere the cog does not, the app menu as a drill named for the cog
// it stands in for. A phone renders every one of these menus as a
// bottom sheet with its own stacked sub-sheets, so a submenu is the sheet's own
// idiom rather than a flyout squeezed against an edge.
//
// THE INPUT GROUP IS FIRST, above the subject's actions, because this menu is
// the only permanent home the virtual input's controls have: typing directly in
// the terminal takes the whole bottom bar away, and the way back has to be
// somewhere that never leaves. It is a thumb's reach from the `⋯` that opened
// the sheet, which is where the hand already is.
//
// The anchors differ in where the menu opens from, in what the trigger looks
// like, and in the one row that is about the chrome AROUND the anchor rather
// than about the pane: the Settings drill, which is there only where the cog is
// not. The INPUT group's ITEMS are
// still one home at a time, because what the group contains is the pane's own
// published answer (see `lib/paneInputGroup.ts`) rather than a per-anchor
// decision: two anchors of this one menu show the same rows, and the split that
// matters is between this top menu and the input `⋯` down in the bottom bar.
// WHICH pane's answer is a question the anchor does have to hand over, because
// only it knows what is on screen underneath it; see `PaneMenuPane`.
//
// There is no "Changes ±N" row: an agent's flap and pill both carry a real
// count BUTTON beside this trigger, keyboard-reachable and labelled for a
// screen reader, and a second copy of it in the menu was two places for the
// same number to be printed. A terminal has no count at all.
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
  /// Which way the menu opens on a surface wide enough to anchor it: the flap
  /// hangs from the band at the top of the pane, the pill floats at the bottom.
  /// A phone ignores it and renders a sheet.
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
  // THE PANE'S OWN VERBS, when the pane on screen is a companion terminal and
  // the menu around it is its agent's. Its Close and its editor entries would
  // otherwise be reachable only from the sidebar row, which is exactly what
  // theater and a narrow window take away. Labelled, and AFTER the agent's
  // actions: the agent body keeps the row order it has at every other anchor,
  // and the label is what keeps Close… from reading as one more agent action.
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
