import {
  Ellipsis,
  Folder,
  FolderPlus,
  Search,
  type LucideIcon,
} from "lucide-react"
import { useId, useRef, useState, type ReactNode } from "react"

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
import { MatchHighlight } from "@/components/MatchHighlight"
import { ModalGroupHeading } from "@/components/modal"
import {
  useProjectSearch,
  type ProjectListRow,
  type ProjectSearch,
} from "@/components/projectSearch"
import { matchCharRanges } from "@/lib/searchMatch"
import { openAddProject } from "@/lib/store"
import { FOCUS_RING, PICK_ROW } from "@/lib/focusRing"
import {
  ALWAYS_REVEALED_ON_PHONE,
  HOVER_REVEAL,
  REVEAL_ROW,
  REVEALED_ON_ANY_FOCUS,
} from "@/lib/hoverReveal"
import { cn } from "@/lib/utils"

/** The rows of a project list, each with the project's `⋯` menu. The caller
 * hands the rows over already in order and puts this in the part that scrolls. */
export function ProjectRows({
  search,
  heading,
  onPick,
  menu,
}: {
  search: ProjectSearch
  /** The heading over the rows, where the list asks the person something. */
  heading?: string
  onPick?: (id: string) => void
  menu: (id: string) => ReactNode
}) {
  const { query, filtered, pick } = search
  // Which row's menu is open. Controlled so a click on the row can open the
  // same menu its `⋯` does.
  const [menuOpenId, setMenuOpenId] = useState<string | null>(null)
  return (
    <>
      {heading ? <ModalGroupHeading>{heading}</ModalGroupHeading> : null}
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
            pickId={pick.rowId(row.id)}
            active={pick.shownKey === row.id}
            menuOpen={menuOpenId === row.id}
            setMenuOpen={(open) => setMenuOpenId(open ? row.id : null)}
            menu={menu}
            needle={query.trim().toLowerCase()}
          />
        ))
      )}
    </>
  )
}

/** The body of the Projects list. The caller renders it inside a
 * `DialogContent` laid out as a flex column. */
export function ProjectList({
  title,
  description,
  rows,
  menu,
}: {
  title: string
  description: string
  rows: ProjectListRow[]
  menu: (id: string) => ReactNode
}) {
  const search = useProjectSearch(rows, undefined)
  return (
    <>
      <DialogHeader className="shrink-0 p-4 pb-3">
        <DialogTitle>{title}</DialogTitle>
        <DialogDescription className="text-pretty">{description}</DialogDescription>
        <div className="mt-2 flex items-center gap-2 rounded-md border border-input bg-input/30 px-3 transition-colors has-[input:focus-visible]:border-ring has-[input:focus-visible]:ring-3 has-[input:focus-visible]:ring-ring/50 max-md:min-h-10 pointer-coarse:min-h-10">
          <Search className="size-4 shrink-0 text-muted-foreground" />
          <input
            value={search.query}
            onChange={(event) => search.setQuery(event.target.value)}
            placeholder="Search projects"
            aria-label="Search projects"
            {...search.pick.fieldProps}
            className="min-w-0 flex-1 bg-transparent py-2 text-sm outline-none placeholder:text-muted-foreground"
            autoFocus
          />
        </div>
      </DialogHeader>

      {/* A fixed height, so the dialog does not resize as a search narrows the
          rows, and the one child that shrinks when the popup is capped. */}
      <ScrollArea className="h-72 min-h-0 shrink border-t">
        <div className="p-2">
          <ProjectRows search={search} menu={menu} />
        </div>
      </ScrollArea>

      <div className="grid shrink-0 grid-cols-1 border-t p-2">
        <FooterButton title="Add a new project…" icon={FolderPlus} onSelect={openAddProject} />
      </div>
    </>
  )
}

interface FooterAction {
  title: string
  icon: LucideIcon
  onSelect: () => void
}

// 36px under a fine pointer, whose only neighbour is the list above, behind a
// border. The transparent border is what the focus ring's border half colours.
function FooterButton({ title, icon: Icon, onSelect }: FooterAction) {
  return (
    <button
      type="button"
      onClick={onSelect}
      className={cn(
        "flex h-9 min-w-0 items-center gap-2.5 rounded-md border border-transparent px-[7px] text-left text-sm text-muted-foreground transition-colors hover:bg-accent/60 hover:text-foreground max-md:h-10 pointer-coarse:h-10",
        FOCUS_RING,
      )}
    >
      <Icon className="size-4 shrink-0" />
      <span className="min-w-0 truncate">{title}</span>
    </button>
  )
}

// One row: the project's own button and its `⋯`. Where the list has no `onPick`,
// the button opens the same menu the `⋯` does, and says so to assistive tech.
function ProjectRow({
  row,
  onPick,
  pickId,
  active,
  menuOpen,
  setMenuOpen,
  menu,
  needle,
}: {
  row: ProjectListRow
  onPick?: (id: string) => void
  pickId: string
  // The row Enter in the search field would pick, while the field has focus.
  active: boolean
  menuOpen: boolean
  setMenuOpen: (open: boolean) => void
  menu: (id: string) => ReactNode
  // The filter's normalized query, lit in the name it matched.
  needle: string
}) {
  const nameId = useId()
  const labelId = useId()
  const detailId = useId()
  const triggerRef = useRef<HTMLButtonElement>(null)
  // Whether the menu's last close came from pressing one of its items.
  const closedByItem = useRef(false)
  // Whether this row's menu was open when the name button was last pressed. The
  // press is an outside press to the menu, which closes it before the click
  // runs, so the click cannot tell "open, close it" from "closed, open it".
  const openAtPress = useRef(false)
  const opensMenu = onPick === undefined
  const describedBy = [row.label ? labelId : null, row.detail ? detailId : null]
    .filter((id): id is string => id !== null)
    .join(" ")

  return (
    <div
      data-testid="project-row"
      data-name={row.name}
      data-active={active ? "" : undefined}
      className={cn(
        "flex items-center gap-2 rounded-md px-2 transition-colors max-md:min-h-10 pointer-coarse:min-h-10",
        REVEAL_ROW,
        "hover:bg-accent/60 has-[[data-popup-open]]:bg-accent/60",
        active && PICK_ROW,
      )}
    >
      <button
        type="button"
        id={pickId}
        // Named by the project alone; the count and the second line describe
        // it, so a screen reader says the name first and the rest after.
        aria-labelledby={nameId}
        aria-describedby={describedBy || undefined}
        aria-haspopup={opensMenu ? "menu" : undefined}
        aria-expanded={opensMenu ? menuOpen : undefined}
        // A second press while the menu is open closes it rather than closing
        // and reopening it.
        // Captured so it reads the state before the menu's outside-press
        // handler closes it.
        onPointerDownCapture={() => {
          openAtPress.current = menuOpen
        }}
        onKeyDown={() => {
          openAtPress.current = false
        }}
        onClick={() => {
          if (onPick) return onPick(row.id)
          const wasOpen = openAtPress.current || menuOpen
          openAtPress.current = false
          setMenuOpen(!wasOpen)
        }}
        // The shared Button's focus ring, so a keyboard user can see which
        // project Enter would pick. The row's own px-2 and the list's p-2 leave
        // the ring room on every side. It stretches to the row's height, so a
        // finger's 40px row is a 40px target and rows meet with no gap.
        className={cn(
          "flex min-w-0 flex-1 items-center gap-2.5 self-stretch rounded-md border border-transparent py-2 text-left",
          FOCUS_RING,
        )}
      >
        <Folder className="mt-0.5 size-4 shrink-0 self-start text-muted-foreground" />
        <span className="flex min-w-0 flex-1 flex-col gap-0.5">
          <span className="flex min-w-0 items-center gap-2">
            <span id={nameId} className="min-w-0 flex-1 truncate text-sm">
              <MatchHighlight text={row.name} ranges={matchCharRanges(row.name, [needle])} />
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
        {/* The shared reveal, also on mouse focus anywhere in the row, and
            always shown at phone width. The cap is the 40px trigger's. */}
        <div
          className={cn(
            HOVER_REVEAL,
            "[--reveal-cap:--spacing(10)]",
            REVEALED_ON_ANY_FOCUS,
            ALWAYS_REVEALED_ON_PHONE,
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
          // took it. Escape returns focus to the trigger; an outside press that
          // landed on something focusable leaves focus there.
          finalFocus={() => {
            if (!closedByItem.current) return focusBelongsToMenuOrNothing()
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

// True when focus is still on the menu or on nothing (Escape, or a click on bare
// page), so it goes back to the trigger; false when a click moved it to another
// control, which keeps it.
function focusBelongsToMenuOrNothing(): boolean {
  const active = document.activeElement
  if (!active || active === document.body) return true
  return active.closest('[data-slot="dropdown-menu-content"]') !== null
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
