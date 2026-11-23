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
  ChevronRight,
  Copy,
  Ellipsis,
  Plus,
  Search,
  SquareSlash,
  Trash2,
  TriangleAlert,
} from "lucide-react"

import { unsavedChangesLabel } from "@/lib/unsavedChanges"
import { LoadingRow } from "@/components/Spinner"
import { MatchHighlight } from "@/components/MatchHighlight"
import {
  Modal,
  useHeldWhileClosed,
  useModalLifetime,
  type ModalCloseReason,
} from "@/components/modal"
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
import { chip, proseText, renderProse, type Prose } from "@/lib/prose"
import { matchCharRanges } from "@/lib/searchMatch"
import {
  closeMacrosDialog,
  dismissMacrosConflict,
  macrosDialogReturnFocus,
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

// Each mark one unbreakable box, which an ellipsis hides whole, so a hidden
// match leaves no sliver of its colour beside the ellipsis.
const WHOLE_MARKS = "[&_mark]:inline-block"

const DESCRIPTION = "Text you send to an agent or a terminal from its Macros… button."

// The macro editor: list and editor side by side on a computer; on a phone the
// list, and the editor as a second step of the same cover. Nothing is written
// until Save sends the whole list.
export function MacrosDialog() {
  const { macrosDialogOpen } = useDux()
  const life = useModalLifetime(macrosDialogOpen)
  return life.mounted ? (
    <MacrosEditor key={life.epoch} open={macrosDialogOpen} onClosed={life.onClosed} />
  ) : null
}

// State that starts over whenever `session` changes: the editor is reseeded in
// place when its draft is reloaded, with the modal around it staying open.
function useSessionState<T>(
  session: string,
  init: () => T,
): [T, (next: T | ((prev: T) => T)) => void] {
  const [held, setHeld] = useState(() => ({ session, value: init() }))
  let now = held
  if (held.session !== session) {
    now = { session, value: init() }
    setHeld(now)
  }
  return [
    now.value,
    (next) =>
      setHeld((prev) => ({
        session: prev.session,
        value: typeof next === "function" ? (next as (prev: T) => T)(prev.value) : next,
      })),
  ]
}

function MacrosEditor({ open, onClosed }: { open: boolean; onClosed: () => void }) {
  const state = useDux()
  const { bootstrap, macrosDraft } = state
  // Closing clears it in the store; held, the notice stays while it fades.
  const macrosConflict = useHeldWhileClosed(open, state.macrosConflict)
  const phone = useIsMobile()
  const loading = bootstrap === null
  // One editing session: an opening, or a reload of the draft inside one, once
  // the macros have loaded.
  const session = `${state.macrosDraftEpoch}:${loading ? "loading" : "loaded"}`
  // Read once per session. The dots and the change count compare against the
  // list its edits were made against, and Save sends that list's version.
  const [start] = useSessionState(session, () => {
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
  const [items, setItems] = useSessionState<DraftMacro[]>(session, () => start.items)
  const [selectedKey, setSelectedKey] = useSessionState<number | null>(session, () => start.selected)
  const [query, setQuery] = useSessionState(session, () => "")
  const [filter, setFilter] = useSessionState<MacroFilter>(session, () => "all")
  // On a phone, "edit" is the second step of the cover: its title row carries
  // the way back to the list, and the browser's Back takes it too.
  const [step, setStep] = useSessionState<"list" | "edit">(session, () => start.step)
  const [deleteKey, setDeleteKey] = useSessionState<number | null>(session, () => null)
  // The macros added here that the user has typed into: one not typed into
  // yet is not called nameless before anything was asked of it.
  const [touched, setTouched] = useSessionState<ReadonlySet<number>>(session, () => new Set())
  // Bumped to put the caret in the name field of the selected macro.
  const [focusName, setFocusName] = useState(0)
  const nameRef = useRef<HTMLInputElement | null>(null)
  const searchRef = useRef<HTMLInputElement | null>(null)
  const emptyNewRef = useRef<HTMLButtonElement | null>(null)
  const rowEls = useRef(new Map<number, HTMLLIElement>())
  // The row a press just selected, which is already where the pointer is.
  const pressed = useRef<number | null>(null)
  // The row still to be brought into view, for when it is not drawn yet.
  const owedScroll = useRef<number | null>(null)
  // The row focus lands on when the list comes back: the one the phone's
  // editor opened from, or the neighbour of a deleted macro.
  const listFocus = useRef<number | null>(start.picked ? start.selected : null)
  // Whether the phone's editor opens with the caret in the name (a new macro).
  const editorFocusesName = useRef(false)
  // Set by a confirmed delete, for the confirm's close to read.
  const deleted = useRef(false)

  // State rather than a ref: the dialog's portal draws the room a commit after
  // this component, so the measuring effect must run again when it arrives.
  const [listRoom, setListRoom] = useState<HTMLDivElement | null>(null)

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
    if (focusName > 0) nameRef.current?.focus()
  }, [focusName])

  // A selection made anywhere but on its own row (opening on a macro, Show the
  // macro, the arrow keys) brings the row into view.
  useEffect(() => {
    const byPress = pressed.current === selectedKey
    pressed.current = null
    owedScroll.current = byPress ? null : selectedKey
    const el = selectedKey === null ? undefined : rowEls.current.get(selectedKey)
    if (el) scrollOwed(selectedKey, el)
  }, [selectedKey])

  function scrollOwed(key: number | null, el: HTMLElement) {
    if (key === null || owedScroll.current !== key) return
    owedScroll.current = null
    el.scrollIntoView?.({ block: "nearest" })
  }

  // The macro Save's hint is about stays listed whatever the search or the
  // filter, so the hint never speaks of a macro nobody can find.
  function visibleOf(list: DraftMacro[], q: string, f: MacroFilter): DraftMacro[] {
    const needle = searchNeedle(q)
    const blocking = validateMacros(draftToMacros(list))
    const blockingKey = blocking === null ? null : list[blocking.index].key
    return list.filter(
      (item) =>
        item.key === blockingKey ||
        (matchesFilter(item.surface, f) && matchesSearch(item, needle)),
    )
  }
  const visible = visibleOf(items, query, filter)
  const listHeight = useWholeRowsHeight(listRoom, visible.length)

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
    pressed.current = key
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
    if (phone) openEditor(key, true)
    else setFocusName((n) => n + 1)
  }

  // The row focus should land on when the list is on screen, else the empty
  // state's New macro.
  function listFocusRow(): HTMLElement | null {
    const key = listFocus.current
    const row = key === null ? undefined : rowEls.current.get(key)
    return row ?? emptyNewRef.current
  }

  // What takes focus when the modal opens or changes step; null leaves it to
  // the modal, which on a computer picks the search field.
  function initialFocus(): HTMLElement | null {
    // A refused save's notice put focus on its Keep editing as it mounted:
    // reloading throws the edits away, so the focus stays there.
    const active = document.activeElement
    if (active instanceof HTMLElement && active.closest("[data-stale-save]")) return active
    if (!phone) return start.picked ? nameRef.current : emptyNewRef.current
    if (step === "edit") return editorFocusesName.current ? nameRef.current : null
    return listFocusRow() ?? searchRef.current
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
      onRowDrawn={scrollOwed}
      onHighlight={setSelectedKey}
      onActivate={activate}
      onMove={(active, over) => setItems(moveItem(items, active, over))}
    />
  )

  // Search and New share a row, with the filter under them: the head of the
  // computer's list column, and on a phone the row over the list that does
  // not scroll with it.
  const listHead = (
    <>
      <div className="flex items-center gap-2">
        <div
          className={cn(
            "flex min-w-0 flex-1 items-center gap-2 rounded-md border border-input bg-input/30 px-3 transition-colors has-[input:focus-visible]:border-ring has-[input:focus-visible]:ring-3 has-[input:focus-visible]:ring-ring/50",
            CONTROL_HEIGHT,
          )}
        >
          <Search className="size-4 shrink-0 text-muted-foreground" aria-hidden />
          <input
            ref={searchRef}
            value={query}
            onChange={(event) => narrow(event.target.value, filter)}
            placeholder="Search macros"
            aria-label="Search macros"
            className="min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground"
          />
        </div>
        <Button variant="outline" className={cn("shrink-0", CONTROL_HEIGHT)} onClick={addNew}>
          <Plus />
          New
        </Button>
      </div>
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
    </>
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

  const editing = phone && step === "edit" && selected !== null
  // The way to the macro at fault, unless it is the one already on screen.
  const problemOnScreen =
    shownProblem !== null && shownProblem.key === selectedKey && (!phone || editing)
  const hasNotices = Boolean(macrosConflict) || (shownProblem !== null && items.length > 0)
  const notices = hasNotices ? (
    <div className="flex shrink-0 flex-col gap-2">
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
    </div>
  ) : null

  const saveHint = loading
    ? "The macros are still loading."
    : ((problem ? proseText(problem.message) : null) ??
      (changes === 0 ? "Nothing has changed since the macros were saved." : null))
  // With nothing to keep or lose, the way out says Close.
  const quiet = items.length === 0 && changes === 0

  const common = {
    kind: "form",
    open,
    onClosed,
    // Back with edits has set them aside, which closed the dialog already.
    onClose: (reason: ModalCloseReason) => {
      if (reason !== "back" || changes === 0) closeMacrosDialog()
    },
    // On the phone's editor the browser's Back is the title row's: it returns
    // to the list, and only from there does it close.
    beforeClose: (reason: ModalCloseReason) => {
      if (reason !== "back" || !editing) return "close"
      setStep("list")
      return "stay"
    },
    // Back cannot stop to ask, so it sets the edits aside for the next opening.
    unsavedEdits: {
      changes,
      holder: "The macros have",
      leaves: [chip("config.toml")],
      setAside: () => setAsideMacrosDraft(currentEdit(), changes),
    },
    size: "w880",
    height: "h640",
    initialFocus,
    // Where the opener asked, else where the modal chooses.
    finalFocus: () => macrosDialogReturnFocus() ?? true,
    cancelLabel: quiet ? "Close" : "Cancel",
    submit: {
      label: phone && changes > 0 ? `Save ${changes} ${changes === 1 ? "change" : "changes"}` : "Save",
      tone: "primary",
      onSelect: save,
      disabledReason: saveHint,
    },
  } as const

  const confirm = (
    <ConfirmDeleteMacro
      macro={deleting}
      onCancel={() => setDeleteKey(null)}
      onConfirm={confirmDelete}
      finalFocus={() => {
        if (!deleted.current) return true
        deleted.current = false
        return listFocusRow() ?? true
      }}
    />
  )

  if (loading) {
    return (
      <Modal {...common} title="Macros">
        <div className="flex justify-center py-10">
          <LoadingRow>Loading the macros…</LoadingRow>
        </div>
      </Modal>
    )
  }

  if (phone) {
    return (
      <>
        <Modal
          {...common}
          title={editing ? "Edit macro" : "Macros"}
          step={editing ? "edit" : "list"}
          back={editing ? { label: "all macros", onSelect: () => setStep("list") } : undefined}
          toolbar={
            editing || items.length === 0 ? undefined : (
              <div className="flex flex-col gap-2">{listHead}</div>
            )
          }
          footerNote={notices ?? undefined}
        >
          {editing ? editor : items.length === 0 ? empty : list}
        </Modal>
        {confirm}
      </>
    )
  }

  return (
    <>
      <Modal
        {...common}
        title="Macros"
        description={DESCRIPTION}
        footerNote={
          <span aria-live="polite">{changes > 0 ? unsavedChangesLabel(changes) : null}</span>
        }
      >
        {items.length === 0 ? (
          <div className="flex min-h-0 flex-1 flex-col">{empty}</div>
        ) : (
          <div className="grid min-h-0 flex-1 grid-cols-[18rem_minmax(0,1fr)] gap-4">
            <div className="flex min-h-0 flex-col gap-2">
              {listHead}
              {/* The scrollbar keeps clear of the rows: a stable gutter, pulled
                  into the column gap so the rows still line up with the search. */}
              <div ref={setListRoom} className="flex min-h-0 flex-1 flex-col">
                <div
                  className="-mr-2 flex min-h-0 flex-col overflow-y-auto pr-2 [scrollbar-gutter:stable]"
                  style={{ height: listHeight }}
                >
                  {list}
                </div>
              </div>
            </div>
            <div className="flex min-h-0 flex-col overflow-y-auto rounded-lg border p-4">
              {editor}
            </div>
          </div>
        )}
        {notices}
      </Modal>
      {confirm}
    </>
  )
}

// The computer's list box height: as many whole rows as the room holds, so at
// rest no row is cut in half; undefined until a row is drawn to measure.
function useWholeRowsHeight(
  el: HTMLElement | null,
  rowCount: number,
): number | undefined {
  const [height, setHeight] = useState<number | undefined>(undefined)
  useEffect(() => {
    if (el === null || typeof ResizeObserver === "undefined") return
    const measure = () => {
      const row = el.querySelector<HTMLElement>('[role="option"]')
      const list = row?.parentElement
      if (!row || !list) {
        setHeight(undefined)
        return
      }
      const gap = parseFloat(getComputedStyle(list).rowGap) || 0
      const pitch = row.offsetHeight + gap
      if (pitch <= 0) return
      const rows = Math.max(1, Math.floor((el.clientHeight + gap) / pitch))
      setHeight(rows * pitch - gap)
    }
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(el)
    return () => observer.disconnect()
  }, [el, rowCount])
  return height
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
      {/* Each choice is the group's height less its 3px padding at both
          edges, 34px under a finger; its invisible margin there reaches the
          group's own edges, a 40px tap area. Across, the neighbours are the
          other choices of this one control, which meet edge to edge. */}
      {options.map((option) => (
        <Radio.Root
          key={option.value}
          value={option.value}
          className="relative inline-flex after:absolute after:inset-x-0 after:-inset-y-[3px] after:hidden max-md:after:block pointer-coarse:after:block h-full flex-1 cursor-default items-center justify-center gap-1.5 rounded-md border border-transparent px-2.5 text-sm font-medium whitespace-nowrap text-foreground/60 transition-all outline-none hover:text-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:outline-1 focus-visible:outline-ring data-checked:bg-background data-checked:text-foreground data-checked:shadow-sm dark:text-muted-foreground dark:hover:text-foreground dark:data-checked:border-input dark:data-checked:bg-input/30 dark:data-checked:text-foreground"
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
  onRowDrawn,
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
  onRowDrawn: (key: number, el: HTMLLIElement) => void
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
                if (el) {
                  rowEls.current.set(item.key, el)
                  onRowDrawn(item.key, el)
                } else rowEls.current.delete(item.key)
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
            <span className={cn("truncate font-medium", WHOLE_MARKS)}>
              <MatchHighlight text={item.name} ranges={matchCharRanges(item.name, needles)} />
            </span>
          )}
        </div>
        <span className={cn("truncate font-mono text-xs text-muted-foreground", WHOLE_MARKS)}>
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

// Delete only takes the macro out of the draft; Save is what writes it. A
// sibling of the editor's modal, never inside it: a dialog nested in another
// draws no overlay of its own.
function ConfirmDeleteMacro({
  macro: live,
  onCancel,
  onConfirm,
  finalFocus,
}: {
  macro: DraftMacro | null
  onCancel: () => void
  onConfirm: () => void
  finalFocus: () => HTMLElement | boolean
}) {
  // The macro it asked about, kept while the close animates.
  const [last, setLast] = useState(live)
  if (live !== null && live !== last) setLast(live)
  const macro = live ?? last
  const name = macro?.name.trim() ?? ""
  return (
    <Modal
      kind="confirm"
      open={live !== null}
      onClose={onCancel}
      title="Delete this macro?"
      description={
        <>
          {name !== "" ? <InlineCode>{name}</InlineCode> : "This untitled macro"}{" "}
          {macro !== null && macro.origin !== null ? (
            <>
              leaves the list now and from <InlineCode>config.toml</InlineCode> when
              you save.
            </>
          ) : (
            "leaves the list now. It was never saved, so nothing else changes."
          )}
        </>
      }
      cancelLabel="Cancel"
      confirm={{ label: "Delete", tone: "destructive", onSelect: onConfirm }}
      finalFocus={finalFocus}
    />
  )
}

// A save the server refused because the list changed after the dialog read it.
// Reloading throws these edits away, so "Keep editing" has focus.
function StaleSaveNotice({ message }: { message: string }) {
  return (
    <div
      role="alert"
      data-stale-save=""
      className="flex shrink-0 flex-col gap-3 rounded-md border border-destructive/50 bg-destructive/10 px-3 py-2 text-sm text-foreground"
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
