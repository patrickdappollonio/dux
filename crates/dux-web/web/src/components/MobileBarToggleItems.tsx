import { Keyboard, KeyboardOff, PanelTopClose, PanelTopOpen } from "lucide-react"

import {
  DropdownMenuItem,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu"
import { useIsMobile } from "@/hooks/use-mobile"
import {
  mobileAccessoryBarVisible,
  mobileTopBarVisible,
  setMobileBarVisibility,
  useDux,
} from "@/lib/store"

// The two mobile-bar quick toggles (`ui.mobile_top_bar`, `ui.mobile_accessory_bar`)
// plus their trailing separator, shared by every mobile terminal screen's ⋯ menu:
// the agent screen's (AgentActionsMenu, context="terminal") and the agentless
// project/standalone screens' (MobileShell). One component so the labels, icons
// and store actions can never drift between the menus.
//
// Neutral color and no trailing ellipsis: they act immediately (an optimistic
// override plus the generic settings PATCH), no dialog and nothing destructive.
// Restore lives on the show-bars button below the terminal and in Preferences.
//
// Self-gated on the viewport: the chrome these toggles hide is mobile-only, so
// a desktop viewport must never see them even when a terminal-screen menu
// renders. Callers add their own context gate where the menu is shared with
// non-terminal surfaces (AgentActionsMenu's `context === "terminal"`).
export function MobileBarToggleItems() {
  const duxState = useDux()
  const topBarVisible = mobileTopBarVisible(duxState)
  const accessoryBarVisible = mobileAccessoryBarVisible(duxState)
  const isMobile = useIsMobile()
  if (!isMobile) return null
  return (
    <>
      <DropdownMenuItem
        onClick={() => void setMobileBarVisibility("top", !topBarVisible)}
      >
        {topBarVisible ? <PanelTopClose /> : <PanelTopOpen />}
        {topBarVisible ? "Hide top bar" : "Show top bar"}
      </DropdownMenuItem>
      <DropdownMenuItem
        onClick={() =>
          void setMobileBarVisibility("accessory", !accessoryBarVisible)
        }
      >
        {accessoryBarVisible ? <KeyboardOff /> : <Keyboard />}
        {accessoryBarVisible ? "Hide terminal keys" : "Show terminal keys"}
      </DropdownMenuItem>
      <DropdownMenuSeparator />
    </>
  )
}
