import { useEffect, useRef, useState } from "react"
import type { CSSProperties, KeyboardEvent, ReactNode, RefObject } from "react"
import { Radio } from "@base-ui/react/radio"
import { RadioGroup } from "@base-ui/react/radio-group"
import {
  DndContext,
  MouseSensor,
  TouchSensor,
  closestCenter,
  useSensor,
  useSensors,
} from "@dnd-kit/core"
import type { DragEndEvent } from "@dnd-kit/core"
import {
  SortableContext,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable"
import {
  ArrowUpToLine,
  ChevronLeft,
  ChevronRight,
  Copy,
  Ellipsis,
  Plus,
  Search,
  SquareSlash,
  Trash2,
  TriangleAlert,
  X,
} from "lucide-react"

import { ExplainedWhenDisabled } from "@/components/ExplainedWhenDisabled"
import { MatchHighlight } from "@/components/MatchHighlight"
import {
  addItem,
  changeCount,
  draftToEdit,
  draftToMacros,
  duplicateItem,
  filterCounts,
  isModified,
  matchesFilter,
  matchesSearch,
  moveItem,
  moveToTop,
  nextKey,
  removeItem,
  searchNeedle,
  seedDraft,
  updateItem,
  type DraftMacro,
  type MacroFilter,
} from "@/components/macrosDraft"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty"
import { InlineCode } from "@/components/ui/inline-code"
import { Input } from "@/components/ui/input"
import { SheetContent, SheetTitle } from "@/components/ui/sheet"
import { tabsListVariants } from "@/components/ui/tabs"
import { Textarea } from "@/components/ui/textarea"
import { useIsMobile } from "@/hooks/use-mobile"
import {
  MOUSE_DRAG_ACTIVATION,
  TOUCH_DRAG_ACTIVATION,
} from "@/lib/dragActivation"
import { FOCUS_RING } from "@/lib/focusRing"
import {
  MACRO_SURFACE_OPTIONS,
  macroTextPreview,
  savedMacrosEdit,
  validateMacros,
} from "@/lib/macros"
import { proseText, renderProse, type Prose } from "@/lib/prose"
import { matchCharRanges } from "@/lib/searchMatch"
import {
  closeMacrosDialog,
  dismissMacrosConflict,
  reloadMacrosDraft,
  saveMacros,
  setAsideMacrosDraft,
  useDux,
} from "@/lib/store"
import type { MacroSurface, MacroView } from "@/lib/types"
import { cn } from "@/lib/utils"

const SURFACE_LABEL: Record<MacroSurface, string> = {
  agent: "Agent",
  terminal: "Terminal",
  both: "Both",
}

const FILTERS: { value: MacroFilter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "agent", label: "Agent" },
  { value: "terminal", label: "Terminal" },
]

// The one height token of the controls that sit together (search, New, the
// filter, the name field and its `⋯`): 32px for a mouse, 40px under a finger.
const CONTROL_HEIGHT = "h-8 max-md:h-10 pointer-coarse:h-10"

const DESCRIPTION = "Text you send to an agent or a terminal from its Macros… button."

function changesLabel(count: number): string {
  return count === 1 ? "1 unsaved change" : `${count} unsaved changes`
}

// How the dialog answers a request to close: the root hands every dismissal to
// the editor, which alone knows whether there is anything to lose.
interface CloseHandler {
  // Cancel, Escape, the close button, a press outside: asks first when there
  // are unsaved changes.
  request: () => void
  // The browser's Back on the list, which closes the dialog and cannot be
  // asked to wait, so unsaved changes are set aside for the next opening.
  back: () => void
}

// The macro editor: list and editor side by side on a computer; on a phone the
// list, and the editor as a second sheet over it. Nothing is written until
// Save sends the whole list.
export function MacrosDialog() {
  const { macrosDialogOpen, macrosDraftEpoch, bootstrap } = useDux()
  const isMobile = useIsMobile()
  const closeRef = useRef<CloseHandler | null>(null)

  return (
    <Dialog
      open={macrosDialogOpen}
      onOpenChange={(open, details) => {
        if (open) return
        const editor = closeRef.current
        if (editor === null) {
          closeMacrosDialog()
          return
        }
        // Back is the shared primitive's imperative close; it keeps asking
        // until the dialog is gone, so it cannot stop at a question.
        if (details.reason === "imperative-action") {
          editor.back()
          return
        }
        details.cancel()
        editor.request()
      }}
    >
      {macrosDialogOpen ? (
        bootstrap === null ? (
          <Shell phone={isMobile}>
            <p className="py-10 text-center text-sm text-muted-foreground">
              Loading the macros…
            </p>
          </Shell>
        ) : (
          <MacrosEditor key={macrosDraftEpoch} phone={isMobile} closeRef={closeRef} />
        )
      ) : null}
    </Dialog>
  )
}

// A dialog on a computer, a bottom sheet on a phone, both under the one dialog
// root, so a rotation swaps the shell without remounting the editor's draft.
function Shell({
  phone,
  initialFocus,
  finalFocus,
  children,
}: {
  phone: boolean
  initialFocus?: () => HTMLElement | boolean | null
  finalFocus?: () => HTMLElement | boolean | null
  children: ReactNode
}) {
  if (phone) {
    return (
      <SheetContent
        side="bottom"
        showCloseButton={false}
        initialFocus={initialFocus}
        finalFocus={finalFocus}
        fullHeight
        className="gap-0 rounded-t-2xl pb-[env(safe-area-inset-bottom)]"
      >
        {children}
      </SheetContent>
    )
  }
  return (
    <DialogContent
      initialFocus={initialFocus}
      className="flex h-[min(40rem,calc(100dvh-2rem))] flex-col gap-4 sm:max-w-[55rem]"
    >
      {children}
    </DialogContent>
  )
}

function MacrosEditor({
  phone,
  closeRef,
}: {
  phone: boolean
  closeRef: RefObject<CloseHandler | null>
}) {
  const { bootstrap, macrosDraft, macrosConflict } = useDux()
  // Read once: the editing session this mounting opens. The dots and the
  // change count compare against the list its edits were made against, and
  // Save sends that list's version.
  const [start] = useState(() => {
    const edit =
      macrosDraft ??
      savedMacrosEdit(bootstrap?.macros ?? [], bootstrap?.macros_version ?? null)
    const items = seedDraft(edit)
    const picked = edit.selected === null ? undefined : items[edit.selected]
    return {
      baseline: edit.baseline,
      version: edit.version,
      items,
      picked: picked !== undefined,
      selected: picked?.key ?? items[0]?.key ?? null,
      step: picked && phone ? ("edit" as const) : ("list" as const),
    }
  })
  const { baseline, version } = start
  const [items, setItems] = useState<DraftMacro[]>(start.items)
  const [selectedKey, setSelectedKey] = useState<number | null>(start.selected)
  const [query, setQuery] = useState("")
  const [filter, setFilter] = useState<MacroFilter>("all")
  // On a phone, "edit" is the second step, a sheet of its own over the list,
  // so it holds a history entry of its own and Back returns to the list.
  const [step, setStep] = useState<"list" | "edit">(start.step)
  const [deleteKey, setDeleteKey] = useState<number | null>(null)
  const [discardOpen, setDiscardOpen] = useState(false)
  // The macros added here that the user has typed into: one not typed into
  // yet is not called nameless before anything was asked of it.
  const [touched, setTouched] = useState<ReadonlySet<number>>(() => new Set())
  // Bumped to put the caret in the name field of the selected macro.
  const [focusName, setFocusName] = useState(0)
  const nameRef = useRef<HTMLInputElement | null>(null)
  const newRef = useRef<HTMLButtonElement | null>(null)
  const backRef = useRef<HTMLButtonElement | null>(null)
  const emptyNewRef = useRef<HTMLButtonElement | null>(null)
  const rowEls = useRef(new Map<number, HTMLLIElement>())
  // The row focus lands on when the list comes back: the one the phone's
  // editor opened from, or the neighbour of a deleted macro.
  const listFocus = useRef<number | null>(null)
  // Whether the phone's editor opens with the caret in the name (a new macro).
  const editorFocusesName = useRef(false)
  // Set by a confirmed delete, for the confirm's close to read.
  const deleted = useRef(false)

  const changes = changeCount(items, baseline)
  const problem = validateMacros(draftToMacros(items))
  const shownProblem = visibleProblem(items, touched)
  const selected = items.find((item) => item.key === selectedKey) ?? null

  // The session as it stands. On a phone's list no macro is on screen, so
  // the dialog comes back on the list.
  function currentEdit() {
    return draftToEdit(items, baseline, version, phone && step === "list" ? null : selectedKey)
  }

  useEffect(() => {
    closeRef.current = {
      request: () => (changes > 0 ? setDiscardOpen(true) : closeMacrosDialog()),
      back: () => (changes > 0 ? setAsideMacrosDraft(currentEdit(), changes) : closeMacrosDialog()),
    }
    return () => {
      closeRef.current = null
    }
  })

  useEffect(() => {
    if (focusName > 0) nameRef.current?.focus()
  }, [focusName])

  function visibleOf(list: DraftMacro[], q: string, f: MacroFilter): DraftMacro[] {
    const needle = searchNeedle(q)
    return list.filter((item) => matchesFilter(item.surface, f) && matchesSearch(item, needle))
  }
  const visible = visibleOf(items, query, filter)

  // Only a search or filter moves the selection off a macro it hides; an edit
  // that takes the macro out of the list leaves it selected.
  function narrow(q: string, f: MacroFilter) {
    setQuery(q)
    setFilter(f)
    const shown = visibleOf(items, q, f)
    if (!shown.some((item) => item.key === selectedKey)) {
      setSelectedKey(shown[0]?.key ?? null)
    }
  }

  function openEditor(key: number, withName = false) {
    listFocus.current = key
    editorFocusesName.current = withName
    setStep("edit")
  }

  // A click, a tap, or Enter on a row: on a phone it opens the macro.
  function activate(key: number) {
    setSelectedKey(key)
    if (phone) openEditor(key)
  }

  function addNew() {
    const surface = filter === "terminal" ? "terminal" : "agent"
    const key = nextKey(items)
    setItems(addItem(items, surface))
    setQuery("")
    setSelectedKey(key)
    if (phone) openEditor(key, true)
    setFocusName((n) => n + 1)
  }

  function edit(key: number, patch: Partial<Pick<DraftMacro, "name" | "text" | "surface">>) {
    setItems(updateItem(items, key, patch))
    if (!touched.has(key)) setTouched(new Set(touched).add(key))
  }

  function duplicate(key: number) {
    const copy = nextKey(items)
    setItems(duplicateItem(items, key))
    setSelectedKey(copy)
    listFocus.current = copy
    setFocusName((n) => n + 1)
  }

  function confirmDelete() {
    if (deleteKey === null) return
    const at = visible.findIndex((item) => item.key === deleteKey)
    const rest = visible.filter((item) => item.key !== deleteKey)
    const next = rest[Math.min(Math.max(at, 0), rest.length - 1)]
    setItems(removeItem(items, deleteKey))
    if (selectedKey === deleteKey) setSelectedKey(next?.key ?? null)
    listFocus.current = next?.key ?? null
    deleted.current = true
    setDeleteKey(null)
    if (phone) setStep("list")
  }

  // Selects the macro a validation problem names, bringing it into the list
  // when the search or the filter hides it.
  function showProblem(key: number) {
    if (!visible.some((item) => item.key === key)) {
      setQuery("")
      setFilter("all")
    }
    setSelectedKey(key)
    if (phone) openEditor(key)
  }

  // Where focus goes when the list is back on screen: the row it should land
  // on, else the empty state's New macro, else the dialog's own choice.
  function listFocusTarget(): HTMLElement | boolean {
    const key = listFocus.current
    const row = key === null ? undefined : rowEls.current.get(key)
    return row ?? emptyNewRef.current ?? true
  }

  function save() {
    saveMacros(currentEdit())
  }

  const deleting = items.find((item) => item.key === deleteKey) ?? null
  const emptyText = emptyListText(query, filter)

  const editor = selected ? (
    <MacroFields
      key={selected.key}
      macro={selected}
      phone={phone}
      first={items[0]?.key === selected.key}
      nameRef={nameRef}
      onChange={(patch) => edit(selected.key, patch)}
      onDuplicate={() => duplicate(selected.key)}
      onMoveToTop={() => setItems(moveToTop(items, selected.key))}
      onDelete={() => setDeleteKey(selected.key)}
    />
  ) : (
    <p className="m-auto text-center text-sm text-muted-foreground">{emptyText}</p>
  )

  const list = (
    <MacroList
      items={visible}
      baseline={baseline}
      needle={searchNeedle(query)}
      selectedKey={selectedKey}
      phone={phone}
      emptyText={emptyText}
      rowEls={rowEls}
      onHighlight={setSelectedKey}
      onActivate={activate}
      onMove={(active, over) => setItems(moveItem(items, active, over))}
    />
  )

  const search = (
    <div
      className={cn(
        "flex min-w-0 flex-1 items-center gap-2 rounded-md border border-input bg-input/30 px-3 transition-colors has-[input:focus-visible]:border-ring has-[input:focus-visible]:ring-3 has-[input:focus-visible]:ring-ring/50",
        CONTROL_HEIGHT,
      )}
    >
      <Search className="size-4 shrink-0 text-muted-foreground" aria-hidden />
      <input
        value={query}
        onChange={(event) => narrow(event.target.value, filter)}
        placeholder="Search macros"
        aria-label="Search macros"
        className="min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground"
      />
    </div>
  )

  const filterChoice = (
    <SegmentedChoice
      label="Show macros for"
      value={filter}
      options={FILTERS.map((f) => ({
        value: f.value,
        label: (
          <>
            {f.label}
            <span className="tabular-nums">{filterCounts(items)[f.value]}</span>
          </>
        ),
      }))}
      onChange={(f) => narrow(query, f)}
      className="w-full shrink-0"
    />
  )

  const newButton = (
    <Button
      ref={phone ? newRef : undefined}
      variant="outline"
      className={cn("shrink-0", CONTROL_HEIGHT)}
      onClick={addNew}
    >
      <Plus />
      New
    </Button>
  )

  const empty = (
    <Empty className={cn(!phone && "border")}>
      <EmptyHeader>
        <EmptyMedia variant="icon">
          <SquareSlash />
        </EmptyMedia>
        <EmptyTitle>No macros yet</EmptyTitle>
        <EmptyDescription>
          A macro is text you send with one {phone ? "tap" : "click"}, such as a
          review request you type ten times a day, or a test command for a
          terminal.
        </EmptyDescription>
      </EmptyHeader>
      <EmptyContent>
        <Button
          ref={emptyNewRef}
          className="max-md:min-h-10 pointer-coarse:min-h-10"
          onClick={addNew}
        >
          <Plus />
          New macro
        </Button>
      </EmptyContent>
    </Empty>
  )

  // The way to the macro at fault, unless it is the one already on screen.
  const problemOnScreen =
    shownProblem !== null && shownProblem.key === selectedKey && (!phone || step === "edit")
  const notices = (
    <>
      {macrosConflict ? <StaleSaveNotice message={macrosConflict} /> : null}
      {shownProblem && items.length > 0 ? (
        <div className="flex items-center gap-3">
          <p className="min-w-0 flex-1 text-xs text-destructive">
            {renderProse(shownProblem.message)}
          </p>
          {problemOnScreen ? null : (
            <Button
              variant="link"
              className="h-auto shrink-0 p-0 text-xs max-md:min-h-10 pointer-coarse:min-h-10"
              onClick={() => showProblem(shownProblem.key)}
            >
              Show the macro
            </Button>
          )}
        </div>
      ) : null}
    </>
  )

  const saveHint =
    (problem ? proseText(problem.message) : null) ??
    (changes === 0 ? "Nothing has changed since the macros were saved." : null)
  const saveDisabled = saveHint !== null
  const saveLabel =
    phone && changes > 0 ? `Save ${changes} ${changes === 1 ? "change" : "changes"}` : "Save"
  const saveButton = (
    <ExplainedWhenDisabled
      disabled={saveDisabled}
      hint={saveHint}
      className={cn("rounded-lg", phone && "w-full")}
    >
      <Button disabled={saveDisabled} className={cn(phone && "h-11 w-full")} onClick={save}>
        {saveLabel}
      </Button>
    </ExplainedWhenDisabled>
  )
  const cancelButton = (
    <Button
      variant="outline"
      className={cn(phone && "h-11 w-full")}
      onClick={() => closeRef.current?.request()}
    >
      Cancel
    </Button>
  )
  // With nothing to keep or lose, the one way out is Close.
  const quiet = items.length === 0 && changes === 0

  const confirms = (
    <>
      <ConfirmDeleteMacro
        macro={deleting}
        saved={deleting !== null && deleting.origin !== null}
        onCancel={() => setDeleteKey(null)}
        onConfirm={confirmDelete}
        finalFocus={() => {
          if (!deleted.current) return true
          deleted.current = false
          return listFocusTarget()
        }}
      />
      <ConfirmDiscardMacros
        open={discardOpen}
        changes={changes}
        onKeep={() => setDiscardOpen(false)}
        onDiscard={() => {
          setDiscardOpen(false)
          closeMacrosDialog()
        }}
      />
    </>
  )

  const initialFocus = () => {
    if (phone) return newRef.current ?? true
    return start.picked ? (nameRef.current ?? true) : true
  }

  if (phone) {
    const closeButton = (
      <Button
        variant="ghost"
        size="icon"
        className="size-10 shrink-0"
        aria-label="Close"
        onClick={() => closeRef.current?.request()}
      >
        <X />
      </Button>
    )
    const footer = quiet ? null : (
      <div className="flex shrink-0 flex-col gap-2 border-t bg-muted/50 px-4 pt-3 pb-4">
        {notices}
        {saveButton}
        {cancelButton}
      </div>
    )
    const editing = step === "edit" && selected !== null
    return (
      <Shell phone initialFocus={initialFocus}>
        <div className="flex min-h-16 shrink-0 items-center gap-2 border-b py-3 pr-3 pl-4">
          <SheetTitle className="flex-1">Macros</SheetTitle>
          {items.length > 0 ? newButton : null}
          {closeButton}
        </div>
        {items.length === 0 ? (
          <div className="flex min-h-0 flex-1 flex-col p-4">{empty}</div>
        ) : (
          <>
            <div className="flex shrink-0 flex-col gap-2 px-4 pt-3 pb-1">
              {search}
              {filterChoice}
            </div>
            <div className="flex min-h-0 flex-1 flex-col overflow-y-auto px-2 py-1">{list}</div>
          </>
        )}
        {/* The editor's sheet covers this one and carries its own. */}
        {editing ? null : footer}
        {editing ? null : confirms}
        <Dialog
          open={editing}
          onOpenChange={(open, details) => {
            if (open) return
            // Back is the one close that steps back; Escape and a press
            // outside close the whole dialog, asking first.
            if (details.reason === "imperative-action") {
              setStep("list")
              return
            }
            details.cancel()
            closeRef.current?.request()
          }}
        >
          <SheetContent
            side="bottom"
            showCloseButton={false}
            initialFocus={() =>
              (editorFocusesName.current ? nameRef.current : backRef.current) ?? true
            }
            finalFocus={listFocusTarget}
            fullHeight
            className="gap-0 rounded-t-2xl pb-[env(safe-area-inset-bottom)]"
          >
            <SheetTitle className="sr-only">Edit macro</SheetTitle>
            <div className="flex min-h-16 shrink-0 items-center gap-2 border-b py-3 pr-3 pl-4">
              <Button
                ref={backRef}
                variant="ghost"
                className="h-10 pl-1"
                aria-label="Back to all macros"
                onClick={() => setStep("list")}
              >
                <ChevronLeft />
                Macros
              </Button>
              <div className="flex-1" />
              {closeButton}
            </div>
            <div className="flex min-h-0 flex-1 flex-col overflow-y-auto p-4">{editor}</div>
            {footer}
            {confirms}
          </SheetContent>
        </Dialog>
      </Shell>
    )
  }

  return (
    <Shell phone={false} initialFocus={initialFocus}>
      <DialogHeader className="shrink-0 pr-8">
        <DialogTitle>Macros</DialogTitle>
        <DialogDescription>{DESCRIPTION}</DialogDescription>
      </DialogHeader>
      {items.length === 0 ? (
        <div className="flex min-h-0 flex-1 flex-col">{empty}</div>
      ) : (
        <div className="grid min-h-0 flex-1 grid-cols-[18rem_minmax(0,1fr)] gap-4">
          <div className="flex min-h-0 flex-col gap-2">
            <div className="flex items-center gap-2">
              {search}
              {newButton}
            </div>
            {filterChoice}
            {/* The scrollbar keeps clear of the rows: a stable gutter, pulled
                into the column gap so the rows still line up with the search. */}
            <div className="-mr-2 flex min-h-0 flex-1 flex-col overflow-y-auto pr-2 [scrollbar-gutter:stable]">
              {list}
            </div>
          </div>
          <div className="flex min-h-0 flex-col overflow-y-auto rounded-lg border p-4">
            {editor}
          </div>
        </div>
      )}
      {notices}
      <DialogFooter className="shrink-0 sm:items-center">
        {quiet ? (
          <Button variant="outline" onClick={() => closeRef.current?.request()}>
            Close
          </Button>
        ) : (
          <>
            <span aria-live="polite" className="text-xs text-muted-foreground sm:mr-auto">
              {changes > 0 ? changesLabel(changes) : null}
            </span>
            {cancelButton}
            {saveButton}
          </>
        )}
      </DialogFooter>
      {confirms}
    </Shell>
  )
}

// The problem to show under the list: the first one, leaving out a macro
// added here and not typed into yet, which only Save's own hint mentions.
function visibleProblem(
  items: DraftMacro[],
  touched: ReadonlySet<number>,
): { message: Prose; key: number } | null {
  const shown = items.filter(
    (item) =>
      item.origin !== null || touched.has(item.key) || item.name !== "" || item.text !== "",
  )
  const found = validateMacros(draftToMacros(shown))
  return found === null ? null : { message: found.message, key: shown[found.index].key }
}

// What an empty list says: a search found nothing, or the filter lists no
// macro (with no search, there is none to speak of).
function emptyListText(query: string, filter: MacroFilter): string {
  if (searchNeedle(query) !== "") return "No macro matches the search."
  return filter === "terminal" ? "No macro is shown on terminals." : "No macro is shown on agents."
}

// One choice among a few, drawn as a segmented control: a radio group, so it
// is announced as a single choice and the arrow keys move it.
function SegmentedChoice<T extends string>({
  label,
  labelledBy,
  describedBy,
  value,
  options,
  onChange,
  className,
}: {
  label?: string
  labelledBy?: string
  describedBy?: string
  value: T
  options: { value: T; label: ReactNode }[]
  onChange: (value: T) => void
  className?: string
}) {
  return (
    <RadioGroup
      aria-label={label}
      aria-labelledby={labelledBy}
      aria-describedby={describedBy}
      value={value}
      onValueChange={(next) => {
        const found = options.find((option) => option.value === next)
        if (found) onChange(found.value)
      }}
      className={cn(tabsListVariants(), "flex", CONTROL_HEIGHT, className)}
    >
      {options.map((option) => (
        <Radio.Root
          key={option.value}
          value={option.value}
          className="relative inline-flex h-full flex-1 cursor-default items-center justify-center gap-1.5 rounded-md border border-transparent px-2.5 text-sm font-medium whitespace-nowrap text-foreground/60 transition-all outline-none hover:text-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:outline-1 focus-visible:outline-ring data-checked:bg-background data-checked:text-foreground data-checked:shadow-sm dark:text-muted-foreground dark:hover:text-foreground dark:data-checked:border-input dark:data-checked:bg-input/30 dark:data-checked:text-foreground"
        >
          {option.label}
        </Radio.Root>
      ))}
    </RadioGroup>
  )
}

// A listbox: the arrow keys, Home and End move the selection, which on a
// computer is the macro in the editor; on a phone Enter or a tap opens the
// selected one. A drag reorders the draft only.
function MacroList({
  items,
  baseline,
  needle,
  selectedKey,
  phone,
  emptyText,
  rowEls,
  onHighlight,
  onActivate,
  onMove,
}: {
  items: DraftMacro[]
  baseline: MacroView[]
  needle: string
  selectedKey: number | null
  phone: boolean
  emptyText: string
  rowEls: RefObject<Map<number, HTMLLIElement>>
  onHighlight: (key: number) => void
  onActivate: (key: number) => void
  onMove: (activeKey: number, overKey: number) => void
}) {
  // A mouse drags on a short pull so a click stays a click, and a touch on a
  // hold so the gesture coexists with the list's scroll (lib/dragActivation.ts).
  const sensors = useSensors(
    useSensor(MouseSensor, { activationConstraint: MOUSE_DRAG_ACTIVATION }),
    useSensor(TouchSensor, { activationConstraint: TOUCH_DRAG_ACTIVATION }),
  )

  function handleDragEnd(event: DragEndEvent) {
    if (!event.over) return
    onMove(keyOf(String(event.active.id)), keyOf(String(event.over.id)))
  }

  function handleKeyDown(event: KeyboardEvent<HTMLUListElement>) {
    const at = items.findIndex((item) => item.key === selectedKey)
    const target =
      event.key === "ArrowDown"
        ? items[Math.min(at + 1, items.length - 1)]
        : event.key === "ArrowUp"
          ? items[Math.max(at - 1, 0)]
          : event.key === "Home"
            ? items[0]
            : event.key === "End"
              ? items[items.length - 1]
              : undefined
    if (target === undefined) return
    event.preventDefault()
    onHighlight(target.key)
    rowEls.current.get(target.key)?.focus()
  }

  if (items.length === 0) {
    return <p className="py-6 text-center text-sm text-muted-foreground">{emptyText}</p>
  }

  // The one row in the tab order: the selected one when it is listed.
  const tabStop = items.some((item) => item.key === selectedKey) ? selectedKey : items[0].key

  return (
    <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={handleDragEnd}>
      <SortableContext items={items.map((item) => dragId(item.key))} strategy={verticalListSortingStrategy}>
        <ul
          role="listbox"
          aria-label="Macros"
          className="flex flex-col gap-0.5"
          onKeyDown={handleKeyDown}
        >
          {items.map((item) => (
            <MacroRow
              key={item.key}
              item={item}
              modified={isModified(item, baseline)}
              needle={needle}
              selected={item.key === selectedKey}
              tabbable={item.key === tabStop}
              phone={phone}
              rowRef={(el) => {
                if (el) rowEls.current.set(item.key, el)
                else rowEls.current.delete(item.key)
              }}
              onActivate={() => onActivate(item.key)}
            />
          ))}
        </ul>
      </SortableContext>
    </DndContext>
  )
}

function dragId(key: number): string {
  return `macro-${key}`
}

function keyOf(id: string): number {
  return Number(id.slice("macro-".length))
}

function MacroRow({
  item,
  modified,
  needle,
  selected,
  tabbable,
  phone,
  rowRef,
  onActivate,
}: {
  item: DraftMacro
  modified: boolean
  needle: string
  selected: boolean
  tabbable: boolean
  phone: boolean
  rowRef: (el: HTMLLIElement | null) => void
  onActivate: () => void
}) {
  // The sortable's own attributes are left off: they would make the option a
  // button and describe a keyboard drag this list does not offer.
  const { listeners, setNodeRef, transform, transition, isDragging } = useSortable({
    id: dragId(item.key),
  })
  const style: CSSProperties = {
    // A vertical list: the drag is locked to the Y axis (see AgentFlatRow).
    transform: transform ? `translate3d(0, ${Math.round(transform.y)}px, 0)` : undefined,
    transition,
  }
  const preview = macroTextPreview(item.text)
  const needles = needle === "" ? [] : [needle]

  return (
    <li
      ref={(el) => {
        setNodeRef(el)
        rowRef(el)
      }}
      style={style}
      {...listeners}
      role="option"
      aria-selected={selected}
      tabIndex={tabbable ? 0 : -1}
      onClick={onActivate}
      onKeyDown={(event) => {
        if (phone && (event.key === "Enter" || event.key === " ")) {
          event.preventDefault()
          onActivate()
        }
      }}
      className={cn(
        "flex cursor-default touch-manipulation items-center gap-2 rounded-md border border-transparent px-2 py-1.5 hover:bg-muted/60",
        phone ? "min-h-14" : "min-h-12",
        FOCUS_RING,
        selected && "bg-muted hover:bg-muted",
        isDragging && "relative z-10 bg-muted opacity-80 shadow-lg",
      )}
    >
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <div className="flex min-w-0 items-center gap-1.5">
          {modified ? (
            <>
              <span aria-hidden className="size-1.5 shrink-0 rounded-full bg-primary" />
              <span className="sr-only">Unsaved changes:</span>
            </>
          ) : null}
          {item.name.trim() === "" ? (
            <span className="truncate font-medium text-muted-foreground italic">Untitled macro</span>
          ) : (
            <span className="truncate font-medium">
              <MatchHighlight text={item.name} ranges={matchCharRanges(item.name, needles)} />
            </span>
          )}
        </div>
        <span className="truncate font-mono text-xs text-muted-foreground">
          <MatchHighlight text={preview} ranges={matchCharRanges(preview, needles)} />
        </span>
      </div>
      <span className="shrink-0 text-xs text-muted-foreground">{SURFACE_LABEL[item.surface]}</span>
      {phone ? <ChevronRight aria-hidden className="size-4 shrink-0 text-muted-foreground" /> : null}
    </li>
  )
}

// The selected macro's editor: its name with the `⋯` of its actions, where it
// is shown, and its text. Every keystroke changes only the draft.
function MacroFields({
  macro,
  phone,
  first,
  nameRef,
  onChange,
  onDuplicate,
  onMoveToTop,
  onDelete,
}: {
  macro: DraftMacro
  phone: boolean
  first: boolean
  nameRef: RefObject<HTMLInputElement | null>
  onChange: (patch: Partial<Pick<DraftMacro, "name" | "text" | "surface">>) => void
  onDuplicate: () => void
  onMoveToTop: () => void
  onDelete: () => void
}) {
  const name = macro.name.trim() === "" ? "this macro" : macro.name
  // Delete… opens its confirm as this menu closes; handing focus back to the
  // trigger then would take it from the confirm's Cancel.
  const confirming = useRef(false)
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3">
      <div className="flex flex-col gap-1.5">
        <label htmlFor="macro-name" className="text-sm font-medium text-muted-foreground">
          Name
        </label>
        <div className="flex items-center gap-2">
          <Input
            id="macro-name"
            ref={nameRef}
            value={macro.name}
            onChange={(e) => onChange({ name: e.target.value })}
            placeholder="Review"
            className={CONTROL_HEIGHT}
          />
          <DropdownMenu>
            <DropdownMenuTrigger
              render={
                <Button
                  variant="outline"
                  size="icon"
                  aria-label={`Actions for ${name}`}
                  className="shrink-0 max-md:size-10 pointer-coarse:size-10"
                />
              }
            >
              <Ellipsis />
            </DropdownMenuTrigger>
            <DropdownMenuContent
              align="end"
              finalFocus={() => {
                const keep = confirming.current
                confirming.current = false
                return !keep
              }}
            >
              {phone ? (
                <DropdownMenuGroup>
                  <DropdownMenuLabel>{macro.name.trim() === "" ? "Untitled macro" : macro.name}</DropdownMenuLabel>
                </DropdownMenuGroup>
              ) : null}
              <DropdownMenuItem onClick={onDuplicate}>
                <Copy />
                Duplicate
              </DropdownMenuItem>
              <DropdownMenuItem disabled={first} onClick={onMoveToTop}>
                <ArrowUpToLine />
                Move to top
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem
                onClick={() => {
                  confirming.current = true
                  onDelete()
                }}
              >
                <Trash2 />
                Delete…
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>

      <div className="flex flex-col gap-1.5">
        <span id="macro-surface" className="text-sm font-medium text-muted-foreground">
          Shown on
        </span>
        <SegmentedChoice
          labelledBy="macro-surface"
          describedBy="macro-surface-meaning"
          value={macro.surface}
          options={MACRO_SURFACE_OPTIONS}
          onChange={(surface) => onChange({ surface })}
          className={phone ? "w-full" : "w-fit"}
        />
        <p id="macro-surface-meaning" className="text-xs text-muted-foreground">
          {MACRO_SURFACE_OPTIONS.find((option) => option.value === macro.surface)?.description}
        </p>
      </div>

      <div className="flex min-h-0 flex-1 flex-col gap-1.5">
        <label htmlFor="macro-text" className="text-sm font-medium text-muted-foreground">
          Text
        </label>
        <Textarea
          id="macro-text"
          value={macro.text}
          onChange={(e) => onChange({ text: e.target.value })}
          placeholder="review this code for bugs"
          className="min-h-30 flex-1 resize-none font-mono field-sizing-fixed"
        />
        <p className="text-xs text-muted-foreground">
          Each new line is sent as Alt+Enter, so the macro arrives as one
          prompt; you press Enter to send it.
        </p>
      </div>
    </div>
  )
}

// Delete only takes the macro out of the draft; Save is what writes it.
function ConfirmDeleteMacro({
  macro,
  saved,
  onCancel,
  onConfirm,
  finalFocus,
}: {
  macro: DraftMacro | null
  saved: boolean
  onCancel: () => void
  onConfirm: () => void
  finalFocus: () => HTMLElement | boolean
}) {
  const named = macro !== null && macro.name.trim() !== ""
  return (
    <Dialog
      open={macro !== null}
      onOpenChange={(open) => {
        if (!open) onCancel()
      }}
    >
      <DialogContent showCloseButton={false} destructive finalFocus={finalFocus}>
        <DialogHeader>
          <DialogTitle>Delete this macro?</DialogTitle>
          <DialogDescription>
            {named ? <InlineCode>{macro.name.trim()}</InlineCode> : "This untitled macro"}{" "}
            {saved ? (
              <>
                leaves the list now and from <InlineCode>config.toml</InlineCode> when
                you save.
              </>
            ) : (
              "leaves the list now. It was never saved, so nothing else changes."
            )}
          </DialogDescription>
        </DialogHeader>
        {/* Misclick-safe spacing between the body and the buttons. */}
        <div className="h-2" />
        <DialogFooter>
          <Button variant="outline" autoFocus onClick={onCancel}>
            Cancel
          </Button>
          <Button variant="destructive" onClick={onConfirm}>
            Delete
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

// Closing with unsaved changes asks first: they exist only in this dialog.
function ConfirmDiscardMacros({
  open,
  changes,
  onKeep,
  onDiscard,
}: {
  open: boolean
  changes: number
  onKeep: () => void
  onDiscard: () => void
}) {
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) onKeep()
      }}
    >
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>Discard unsaved changes?</DialogTitle>
          <DialogDescription>
            The macros have {changesLabel(changes)}. Discarding them leaves{" "}
            <InlineCode>config.toml</InlineCode> as it was.
          </DialogDescription>
        </DialogHeader>
        <div className="h-2" />
        <DialogFooter>
          <Button variant="outline" autoFocus onClick={onKeep}>
            Keep editing
          </Button>
          <Button variant="destructive" onClick={onDiscard}>
            Discard changes
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

// A save the server refused because the list changed after the dialog read it.
// Reloading throws these edits away, so "Keep editing" has focus.
function StaleSaveNotice({ message }: { message: string }) {
  return (
    <div
      role="alert"
      className="flex shrink-0 flex-col gap-3 rounded-md border border-destructive/50 bg-destructive/10 px-3 py-2 text-sm"
    >
      <div className="flex items-start gap-2">
        <TriangleAlert className="mt-0.5 size-4 shrink-0 text-destructive" aria-hidden />
        <span>{message}</span>
      </div>
      <div className="flex justify-end gap-3">
        <Button variant="outline" autoFocus onClick={dismissMacrosConflict} className="max-md:min-h-10 pointer-coarse:min-h-10">
          Keep editing
        </Button>
        <Button variant="destructive" onClick={() => void reloadMacrosDraft()} className="max-md:min-h-10 pointer-coarse:min-h-10">
          Reload macros
        </Button>
      </div>
    </div>
  )
}
