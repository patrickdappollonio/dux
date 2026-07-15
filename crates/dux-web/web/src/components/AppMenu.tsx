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
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        render={
          <Button variant="outline" size="icon" aria-label="Menu">
            <Settings />
          </Button>
        }
      />
      <DropdownMenuContent align="end" side="bottom">
        <AppMenuEntries entries={appMenuModel()} />
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
