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

// The desktop app menu: the cog in the header's top-right corner, replacing the
// old "Commands…" command-palette button. It renders `appMenuModel()` and
// hand-authors no items, so it cannot drift from the mobile bottom sheet
// (`AppMenuSheet.tsx`), which renders the same model.
//
// Deliberately NO keyboard shortcut (the web has no Ctrl+K anymore). The cog is
// a plain <button>: Tab reaches it, Enter/Space activate it natively, ArrowDown
// opens it, arrows move within it, Escape closes it and restores focus. All of
// that comes from base-ui's Menu plus the native button.

function AppMenuEntries({ entries }: { entries: AppMenuEntry[] }) {
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

export function AppMenu() {
  // The model's one context input: gh availability gates the from-PR agent
  // variant, exactly as the sidebar's NewAgentSplitButton gates its copy.
  const { bootstrap } = useDux()
  const ghAvailable = bootstrap?.gh_available ?? false
  return (
    <DropdownMenu>
      {/* LABELLED on desktop, where there is room: "Settings" says what the cog
          opens without a hover, and this is the menu every global action lives
          behind. The label changes the WIDTH and nothing else, the button's
          default size token is `h-8`, exactly the `size="icon"` (`size-8`)
          height of the icon-only buttons beside it, so the control row stays one
          height. The `aria-label="Menu"` it used to carry is GONE rather than
          kept: an aria-label overrides the accessible name, so a button reading
          "Settings" would have announced as "Menu" and no voice command matching
          the visible word would reach it. The visible text is the name now. */}
      <DropdownMenuTrigger
        render={
          <Button variant="outline">
            <Settings />
            Settings
          </Button>
        }
      />
      <DropdownMenuContent align="end" side="bottom">
        <AppMenuEntries entries={appMenuModel({ ghAvailable })} />
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
