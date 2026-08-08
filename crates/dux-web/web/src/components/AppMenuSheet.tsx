import { useState } from "react"
import { ChevronLeft, ChevronRight } from "lucide-react"

import { Button } from "@/components/ui/button"
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet"
import { appMenuModel, findSubmenu, type AppMenuEntry } from "@/lib/appMenu"
import { useDux } from "@/lib/store"

// The mobile app menu: the same `appMenuModel()` the desktop flyout renders,
// presented as a bottom sheet with drill-down.
//
// Why a different renderer instead of the desktop's DropdownMenu: a hover flyout
// cannot work on touch. So the MODEL is shared and only the presentation
// differs, which is what keeps the two from drifting (pinned by
// AppMenuSheet.test.tsx's "same top-level titles" test).
//
// ARIA is ours here. The desktop menu gets role/aria-haspopup from base-ui's
// primitives; this is a hand-rolled list, so we supply them. `Sheet` still
// provides the dialog role, the focus trap, and Escape.

function AppMenuRows({
  entries,
  onDrill,
  onRun,
}: {
  entries: AppMenuEntry[]
  onDrill: (id: string) => void
  onRun: (run: () => void) => void
}) {
  return (
    <div role="menu" className="flex flex-col pb-2">
      {entries.map((entry) => {
        if (entry.kind === "separator") {
          return <div key={entry.id} role="separator" className="my-1 h-px bg-border" />
        }
        const Icon = entry.icon
        const isSubmenu = entry.kind === "submenu"
        return (
          <button
            key={entry.id}
            type="button"
            role="menuitem"
            // min-h-11 is the 44px touch floor (CLAUDE.md touch-target tenet);
            // AppMenuSheet.test.tsx pins the class contract.
            className="flex min-h-11 w-full items-center gap-3 px-4 text-left text-sm transition-colors hover:bg-accent focus-visible:bg-accent focus-visible:outline-none motion-reduce:transition-none"
            // A submenu row opens a deeper list rather than a popup, but it is
            // still a menuitem that owns a menu, so aria-haspopup is correct.
            // aria-expanded stays false: the child list REPLACES this one rather
            // than expanding under it, so nothing is expanded in place.
            aria-haspopup={isSubmenu ? "menu" : undefined}
            aria-expanded={isSubmenu ? false : undefined}
            onClick={() =>
              isSubmenu ? onDrill(entry.id) : onRun(entry.run)
            }
          >
            <Icon className="size-4 shrink-0" />
            <span className="flex-1">{entry.title}</span>
            {isSubmenu ? (
              <ChevronRight className="size-4 shrink-0 text-muted-foreground" />
            ) : null}
          </button>
        )
      })}
    </div>
  )
}

export function AppMenuSheet({
  open,
  onOpenChange,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  const [drilled, setDrilled] = useState<string | null>(null)

  // Reopening always starts at the root. Resetting only inside the close handler
  // would miss a close driven by the PARENT (it owns `open`), stranding the next
  // open three levels deep under a header that says "Menu". Keying off the
  // open->closed transition catches both. This is React's documented "adjust
  // state during render" pattern (a prev tracker compared each render, as
  // `NumberControl` in CustomizeWebappDialog.tsx already does) rather than an
  // effect, so the reset lands in the same commit as the prop change.
  const [prevOpen, setPrevOpen] = useState(open)
  if (open !== prevOpen) {
    setPrevOpen(open)
    if (!open) setDrilled(null)
  }

  // The model's one context input: gh availability gates the from-PR agent
  // variant, exactly as the sidebar's NewAgentSplitButton gates its copy.
  const { bootstrap } = useDux()
  const ghAvailable = bootstrap?.gh_available ?? false

  const model = appMenuModel({ ghAvailable })
  const submenu = drilled ? findSubmenu(model, drilled) : null
  // Fall back to the root if a drilled id ever goes missing, so the sheet can
  // never strand the user on an empty list with no way back.
  const entries = submenu ? submenu.entries : model
  const title = submenu ? submenu.title : "Menu"

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent side="bottom" className="max-h-[80vh] overflow-y-auto">
        <SheetHeader className="flex-row items-center gap-2 pb-0">
          {submenu ? (
            <Button
              variant="ghost"
              size="icon"
              aria-label="Back"
              className="size-11 shrink-0"
              onClick={() => setDrilled(null)}
            >
              <ChevronLeft />
            </Button>
          ) : null}
          <SheetTitle>{title}</SheetTitle>
          <SheetDescription className="sr-only">
            dux app menu: preferences, configuration, and agent-wide actions.
          </SheetDescription>
        </SheetHeader>
        <AppMenuRows
          entries={entries}
          onDrill={setDrilled}
          onRun={(run) => {
            run()
            onOpenChange(false)
          }}
        />
      </SheetContent>
    </Sheet>
  )
}
