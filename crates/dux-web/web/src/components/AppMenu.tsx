import { Settings } from "lucide-react"

import { Button } from "@/components/ui/button"
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
import { appMenuModel, type AppMenuEntry } from "@/lib/appMenu"
import { useDux } from "@/lib/store"

// The desktop app menu renders `appMenuModel()` and
// hand-authors no items, so it cannot drift from the mobile bottom sheet
// (`AppMenuSheet.tsx`), which renders the same model.
//
// The BODY is exported separately from the cog, because the header is not the
// only place this menu has to appear: theater mode takes the header and the
// sidebar away, and the floating pill's `⋯` carries the same menu so that
// Preferences and every creation action stay reachable while the mode is on.
// It renders the model and reads the same context, so the two anchors cannot
// offer different things.
//
// Deliberately NO keyboard shortcut (the web has no Ctrl+K anymore). The cog is
// a plain <button>: Tab reaches it, Enter/Space activate it natively, ArrowDown
// opens it, arrows move within it, Escape closes it and restores focus. All of
// that comes from base-ui's Menu plus the native button.

export function AppMenuEntries({ entries }: { entries: AppMenuEntry[] }) {
  return (
    <>
      {entries.map((entry) => {
        if (entry.kind === "separator") {
          return <DropdownMenuSeparator key={entry.id} />
        }
        if (entry.kind === "submenu") {
          const Icon = entry.icon
          return (
            <DropdownMenuSub key={entry.id}>
              <DropdownMenuSubTrigger>
                <Icon />
                {entry.title}
              </DropdownMenuSubTrigger>
              {/* `side="left"` is deliberate and load-bearing: the wrapper
                  defaults submenus to "right", and this menu is anchored to the
                  app's right edge, so a right-opening flyout would run
                  off-screen. base-ui's positioner does collision-flip, but we
                  state the intent rather than depend on the fallback. */}
              <DropdownMenuSubContent side="left">
                <AppMenuEntries entries={entry.entries} />
              </DropdownMenuSubContent>
            </DropdownMenuSub>
          )
        }
        const Icon = entry.icon
        return (
          <DropdownMenuItem key={entry.id} onClick={entry.run}>
            <Icon />
            {entry.title}
          </DropdownMenuItem>
        )
      })}
    </>
  )
}

// Every item of the app menu, ready to drop into any `DropdownMenuContent`.
export function AppMenuBody() {
  // The model's one context input: gh availability gates the from-PR agent
  // variant, exactly as the launcher corner's ⋯ menu gates its copy.
  const { bootstrap } = useDux()
  const ghAvailable = bootstrap?.gh_available ?? false
  const githubIntegrationEnabled = bootstrap?.github_integration ?? false
  return (
    <AppMenuEntries
      entries={appMenuModel({ ghAvailable, githubIntegrationEnabled })}
    />
  )
}

export function AppMenu() {
  return (
    <DropdownMenu>
      {/* LABELLED on desktop, where there is room: "Settings" says what the cog
          opens without a hover, and this is the menu every global action lives
          behind. The label changes the WIDTH and nothing else, the button's
          default size token is `h-8`, exactly the `size="icon"` (`size-8`)
          height of the icon-only buttons beside it, so the control row stays one
          height. Visible text supplies the accessible name so voice commands
          can match "Settings". */}
      <DropdownMenuTrigger
        render={
          <Button variant="outline">
            <Settings />
            Settings
          </Button>
        }
      />
      <DropdownMenuContent align="end" side="bottom">
        <AppMenuBody />
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
