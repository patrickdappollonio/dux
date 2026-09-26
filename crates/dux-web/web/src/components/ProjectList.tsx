import { Ellipsis, Folder, FolderPlus, Search } from "lucide-react"
import { useMemo, useState, type ReactNode } from "react"

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
              <div
                key={row.id}
                data-testid="project-row"
                data-name={row.name}
                className={cn(
                  "group/project-row flex items-center gap-2 rounded-md px-2 transition-colors max-md:min-h-10",
                  "hover:bg-accent/60 has-[[data-popup-open]]:bg-accent/60",
                )}
              >
                <button
                  type="button"
                  onClick={() =>
                    onPick ? onPick(row.id) : setMenuOpenId(row.id)
                  }
                  className="flex min-w-0 flex-1 items-center gap-2.5 py-2 text-left"
                >
                  <Folder className="size-4 shrink-0 self-start text-muted-foreground mt-0.5" />
                  <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                    <span className="flex min-w-0 items-center gap-2">
                      <span className="min-w-0 flex-1 truncate text-sm">
                        {row.name}
                      </span>
                      {row.label ? (
                        <span className="shrink-0 font-mono text-xs text-muted-foreground">
                          {row.label}
                        </span>
                      ) : null}
                    </span>
                    {row.detail}
                  </span>
                </button>
                <DropdownMenu
                  open={menuOpenId === row.id}
                  onOpenChange={(open) => setMenuOpenId(open ? row.id : null)}
                >
                  {/* The row rule: revealed on hover or focus anywhere in the
                      row, kept while its menu is open, always shown on a
                      coarse pointer, and taking no width while idle. */}
                  <div
                    className={cn(
                      "flex shrink-0 items-center overflow-hidden transition-[max-width,opacity] duration-200 ease-out motion-reduce:transition-none max-md:max-w-none md:max-w-0 md:opacity-0 md:group-hover/project-row:max-w-8 md:group-hover/project-row:opacity-100 md:group-focus-within/project-row:max-w-8 md:group-focus-within/project-row:opacity-100 md:has-[[data-popup-open]]:max-w-8 md:has-[[data-popup-open]]:opacity-100",
                      ALWAYS_REVEALED_ON_TOUCH,
                    )}
                  >
                    <DropdownMenuTrigger
                      render={
                        <Button
                          variant="ghost"
                          size="icon"
                          // 32px under a fine pointer, where the only neighbour
                          // on either axis is this row's own button, whose
                          // click opens a dialog at worst (the picker) or this
                          // same menu (the Projects list); the 40px floor
                          // returns on a phone and on any coarse pointer.
                          className="size-8 shrink-0 max-md:size-10 pointer-coarse:size-10"
                          aria-label="Project actions"
                        />
                      }
                    >
                      <Ellipsis />
                    </DropdownMenuTrigger>
                  </div>
                  <DropdownMenuContent align="end">
                    {menu(row.id)}
                  </DropdownMenuContent>
                </DropdownMenu>
              </div>
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
