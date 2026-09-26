import { Ellipsis, Folder, FolderPlus, Search } from "lucide-react"
import { useId, useMemo, useRef, useState, type ReactNode } from "react"

import { Button } from "@/components/ui/button"
import {
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { InlineCode } from "@/components/ui/inline-code"
import { ScrollArea } from "@/components/ui/scroll-area"
import { openAddProject } from "@/lib/store"
import { ALWAYS_REVEALED_ON_TOUCH } from "@/lib/touchReveal"
import { cn } from "@/lib/utils"

/** One row of a project list: a project, or an orphaned group of agents whose
 * project record is gone. */
export interface ProjectListRow {
  id: string
  name: string
  /** Trailing text on the first line (a count), or null for none. */
  label: string | null
  /** An optional second line under the name. */
  detail?: ReactNode
}

/**
 * The body of a dialog that lists projects, shared by the New agent picker and
 * the Projects list so the two cannot drift: the header with its search field,
 * the rows, each with the project's `⋯` menu, and the "Add a new project…"
 * footer. The caller renders it inside a `DialogContent` laid out as a flex
 * column and hands it the rows already in order (both callers freeze the order
 * when they open, so a list that re-sorts under the pointer cannot land a click
 * on the wrong project).
 *
 * `onPick` is what a row click does. Without one, a row click opens that row's
 * menu, which is the same menu its `⋯` opens.
 */
export function ProjectList({
  title,
  description,
  rows,
  onPick,
  menu,
}: {
  title: string
  description: string
  rows: ProjectListRow[]
  onPick?: (id: string) => void
  menu: (id: string) => ReactNode
}) {
  const [query, setQuery] = useState("")
  // Which row's menu is open. Controlled so a click on the row can open the
  // same menu its `⋯` does.
  const [menuOpenId, setMenuOpenId] = useState<string | null>(null)

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase()
    if (q === "") return rows
    return rows.filter((row) => row.name.toLowerCase().includes(q))
  }, [rows, query])

  return (
    <>
      <DialogHeader className="shrink-0 p-4 pb-3">
        <DialogTitle>{title}</DialogTitle>
        <DialogDescription>{description}</DialogDescription>
        <div className="mt-2 flex items-center gap-2 rounded-md border border-input bg-input/30 px-3 max-md:min-h-10">
          <Search className="size-4 shrink-0 text-muted-foreground" />
          <input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Search projects"
            aria-label="Search projects"
            className="min-w-0 flex-1 bg-transparent py-2 text-sm outline-none placeholder:text-muted-foreground"
            autoFocus
          />
        </div>
      </DialogHeader>

      {/* Height h-72 (not max-h) so the modal never grows or shrinks with the
          result count as the user types; the list scrolls internally and the
          empty state fills the same space instead of collapsing. It is a flex
          child that may SHRINK (min-h-0, never grow) so that when the soft
          keyboard shrinks the popup's dvh cap, this list gives up the space
          and the header and Add-project footer stay on screen and tappable. */}
      <ScrollArea className="h-72 min-h-0 shrink border-t">
        <div className="p-2">
          <p className="px-2 pt-1 pb-1.5 font-mono text-xs uppercase tracking-wide text-muted-foreground">
            Choose a project
          </p>
          {filtered.length === 0 ? (
            <p className="px-2 py-4 text-sm text-muted-foreground">
              {query.trim() === "" ? (
                "No projects yet."
              ) : (
                <>
                  No projects match <InlineCode>{query}</InlineCode>.
                </>
              )}
            </p>
          ) : (
            filtered.map((row) => (
              <ProjectRow
                key={row.id}
                row={row}
                onPick={onPick}
                menuOpen={menuOpenId === row.id}
                setMenuOpen={(open) => setMenuOpenId(open ? row.id : null)}
                menu={menu}
              />
            ))
          )}
        </div>
      </ScrollArea>

      <div className="shrink-0 border-t p-2">
        <button
          type="button"
          onClick={openAddProject}
          className="flex w-full items-center gap-2.5 rounded-md px-2 py-2 text-left text-sm text-muted-foreground transition-colors hover:bg-accent/60 hover:text-foreground max-md:min-h-10"
        >
          <FolderPlus className="size-4 shrink-0" />
          Add a new project…
        </button>
      </div>
    </>
  )
}

// One row: the project's own button and its `⋯`. Where the list has no `onPick`,
// the button opens the same menu the `⋯` does, and says so to assistive tech.
function ProjectRow({
  row,
  onPick,
  menuOpen,
  setMenuOpen,
  menu,
}: {
  row: ProjectListRow
  onPick?: (id: string) => void
  menuOpen: boolean
  setMenuOpen: (open: boolean) => void
  menu: (id: string) => ReactNode
}) {
  const nameId = useId()
  const labelId = useId()
  const detailId = useId()
  const triggerRef = useRef<HTMLButtonElement>(null)
  // Whether the menu's last close came from pressing one of its items.
  const closedByItem = useRef(false)
  const opensMenu = onPick === undefined
  const describedBy = [row.label ? labelId : null, row.detail ? detailId : null]
    .filter((id): id is string => id !== null)
    .join(" ")

  return (
    <div
      data-testid="project-row"
      data-name={row.name}
      className={cn(
        "group/project-row flex items-center gap-2 rounded-md px-2 transition-colors max-md:min-h-10",
        "hover:bg-accent/60 has-[[data-popup-open]]:bg-accent/60",
      )}
    >
      <button
        type="button"
        // Named by the project alone; the count and the second line describe
        // it, so a screen reader says the name first and the rest after.
        aria-labelledby={nameId}
        aria-describedby={describedBy || undefined}
        aria-haspopup={opensMenu ? "menu" : undefined}
        aria-expanded={opensMenu ? menuOpen : undefined}
        // A second press while the menu is open closes it rather than closing
        // and reopening it.
        onClick={() => (onPick ? onPick(row.id) : setMenuOpen(!menuOpen))}
        className="flex min-w-0 flex-1 items-center gap-2.5 py-2 text-left"
      >
        <Folder className="mt-0.5 size-4 shrink-0 self-start text-muted-foreground" />
        <span className="flex min-w-0 flex-1 flex-col gap-0.5">
          <span className="flex min-w-0 items-center gap-2">
            <span id={nameId} className="min-w-0 flex-1 truncate text-sm">
              {row.name}
            </span>
            {row.label ? (
              <span
                id={labelId}
                className="shrink-0 font-mono text-xs text-muted-foreground"
              >
                {row.label}
              </span>
            ) : null}
          </span>
          {row.detail ? <span id={detailId}>{row.detail}</span> : null}
        </span>
      </button>
      <DropdownMenu
        open={menuOpen}
        onOpenChange={(open, details) => {
          closedByItem.current = !open && details.reason === "item-press"
          setMenuOpen(open)
        }}
      >
        {/* The row rule: revealed on hover or focus anywhere in the row, kept
            while its menu is open, always shown on a coarse pointer, and taking
            no width while idle. Every revealed cap is max-w-10, the trigger's
            40px coarse-pointer size: these md: caps outrank the unprefixed
            pointer-coarse:max-w-none, so a narrower cap would clip a focused or
            open trigger to 32px on a tablet at desktop width. */}
        <div
          className={cn(
            "flex shrink-0 items-center overflow-hidden transition-[max-width,opacity] duration-200 ease-out motion-reduce:transition-none max-md:max-w-none md:max-w-0 md:opacity-0 md:group-hover/project-row:max-w-10 md:group-hover/project-row:opacity-100 md:group-focus-within/project-row:max-w-10 md:group-focus-within/project-row:opacity-100 md:has-[[data-popup-open]]:max-w-10 md:has-[[data-popup-open]]:opacity-100",
            ALWAYS_REVEALED_ON_TOUCH,
          )}
        >
          <DropdownMenuTrigger
            ref={triggerRef}
            render={
              <Button
                variant="ghost"
                size="icon"
                // 32px under a fine pointer, where the only neighbour on
                // either axis is this row's own button, whose click opens a
                // dialog at worst (the picker) or this same menu (the Projects
                // list); the 40px floor returns on a phone and on any coarse
                // pointer.
                className="size-8 shrink-0 max-md:size-10 pointer-coarse:size-10"
                aria-label="Project actions"
              />
            }
          >
            <Ellipsis />
          </DropdownMenuTrigger>
        </div>
        <DropdownMenuContent
          align="end"
          // Most items open a project dialog over this list, and the menu's
          // usual hand-back to its trigger can land AFTER that dialog took
          // focus, leaving Escape closing the list behind the dialog instead
          // of the dialog. So an item press hands focus back only if nothing
          // took it; Escape and an outside press keep the usual behaviour.
          finalFocus={() => {
            if (!closedByItem.current) return true
            refocusTriggerIfUnclaimed(triggerRef.current)
            return false
          }}
        >
          {menu(row.id)}
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  )
}

// After an item press, hand focus back to the row's trigger only if nothing took
// it: a project dialog the item opened owns focus the moment it has it, and a
// dialog that focuses later still wins, because this only acts on an unfocused
// page. Two frames, so the menu has finished closing first.
function refocusTriggerIfUnclaimed(trigger: HTMLElement | null): void {
  requestAnimationFrame(() =>
    requestAnimationFrame(() => {
      if (!trigger?.isConnected) return
      const active = document.activeElement
      if (active === null || active === document.body) trigger.focus()
    }),
  )
}
