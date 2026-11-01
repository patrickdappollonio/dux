import { useEffect, useId, useRef, useState } from "react"
import { CheckIcon, Code2, SearchIcon, Wand2 } from "lucide-react"

import { MatchHighlight } from "@/components/MatchHighlight"
import { InputGroup, InputGroupAddon } from "@/components/ui/input-group"
import { matchLanguageChoices, type LanguageChoice } from "@/lib/editorLanguage"
import { matchCharRanges } from "@/lib/searchMatch"
import { cn } from "@/lib/utils"

// Scrolls a row into view, the optional call because jsdom has neither layout
// nor scrollIntoView.
function reveal(rowId: string | undefined): void {
  if (rowId === undefined) return
  document.getElementById(rowId)?.scrollIntoView?.({ block: "nearest" })
}

// A row's key: a language's id, or null for Auto.
type RowKey = string | null
const AUTO: RowKey = null

// The highlighted row, and whether it got there by a move the list scrolls to
// (the search or the keyboard) rather than by the pointer, which is already
// over a row the user can see.
interface Highlight {
  row: RowKey
  reveal: boolean
}

// Where the highlight lands when the search becomes `query`: the best match,
// or no row at all when nothing matches. With no search it is the current
// choice, or Auto when that choice is not a language the list has.
function highlightFor(
  query: string,
  choices: readonly LanguageChoice[],
  current: string | undefined,
): Highlight | null {
  if (query.trim() !== "") {
    const best = matchLanguageChoices(choices, query)[0]
    return best === undefined ? null : { row: best.id, reveal: true }
  }
  return current !== undefined && choices.some((c) => c.id === current)
    ? { row: current, reveal: true }
    : { row: AUTO, reveal: true }
}

// The editor's language picker: a search field over every registered
// language, with "Auto" pinned above the matches whatever the query, because
// it is the state every file starts in and the one a user goes looking for to
// undo a pick; it CLEARS the override rather than selecting a language, so it
// is checked exactly when no override is set. Shared by the status bar's
// popover and the phone fold's sheet so the two cannot drift.
//
// A combobox: the field owns the keyboard and points a screen reader at the
// highlighted row through `aria-activedescendant`. The highlight is decided
// here alone. Every search change puts it on the best match, or on no row when
// nothing matches, so Enter cannot quietly undo a pick; an arrow key or the
// pointer moves it until the next search change. Mounted inside the popup, so
// the search starts empty on every open. The filter is `matchLanguageChoices`;
// the lit letters are the query's plain occurrences in the label, which is
// what that filter matched when it matched the label.
export function LanguagePickerList({
  choices,
  current,
  onPick,
  fieldRef,
}: {
  choices: readonly LanguageChoice[]
  // The override in force, or undefined for Auto.
  current: string | undefined
  onPick: (id: string | null) => void
  // The search field, for a popup that must name it as its initial focus.
  fieldRef?: React.Ref<HTMLInputElement>
}) {
  const baseId = useId()
  const listId = `${baseId}-list`
  const rowId = (key: RowKey): string =>
    key === AUTO ? `${baseId}-auto` : `${baseId}-language-${key}`

  const [search, setSearch] = useState("")
  const [highlight, setHighlight] = useState(() =>
    highlightFor("", choices, current),
  )
  const matches = matchLanguageChoices(choices, search)
  const needle = search.trim().toLowerCase()
  const rows: RowKey[] = [AUTO, ...matches.map((c) => c.id)]
  // A highlighted language the list no longer holds (the registry changed
  // under it) highlights no row rather than one that is not there.
  const active =
    highlight !== null && rows.includes(highlight.row) ? highlight : null
  const activeId = active === null ? undefined : rowId(active.row)

  // A highlight the search or the keyboard moved is scrolled into view, and
  // brought back into view whenever the list's box changes size: the popup is
  // placed and height-capped only after it mounts, and a soft keyboard shrinks
  // it later, either of which can undo the first scroll. That second part
  // lasts only until the user scrolls the list themselves (a wheel, a touch
  // drag, a press on the scrollbar) or the pointer takes the highlight, so a
  // list they have moved is never moved for them.
  const scrollerRef = useRef<HTMLDivElement | null>(null)
  const pendingReveal = useRef<string | undefined>(undefined)
  const revealing = active !== null && active.reveal
  useEffect(() => {
    pendingReveal.current = revealing ? activeId : undefined
    if (revealing) reveal(activeId)
  }, [activeId, revealing])
  useEffect(() => {
    const scroller = scrollerRef.current
    if (scroller === null) return
    const cancel = () => {
      pendingReveal.current = undefined
    }
    const events = ["wheel", "touchmove", "pointerdown"] as const
    for (const name of events) {
      scroller.addEventListener(name, cancel, { passive: true })
    }
    const observer =
      typeof ResizeObserver === "function"
        ? new ResizeObserver(() => reveal(pendingReveal.current))
        : null
    observer?.observe(scroller)
    return () => {
      for (const name of events) scroller.removeEventListener(name, cancel)
      observer?.disconnect()
    }
  }, [])

  function changeSearch(next: string): void {
    setSearch(next)
    setHighlight(highlightFor(next, choices, current))
  }

  function move(step: 1 | -1): void {
    // From no highlight, Down lands on the first row and Up on the last.
    const at = active === null ? -1 : rows.indexOf(active.row)
    const next = at === -1 ? (step === 1 ? 0 : rows.length - 1) : at + step
    if (next < 0 || next >= rows.length) return
    setHighlight({ row: rows[next], reveal: true })
  }

  // The keys `useSearchPick` answers to, by its rules: a plain press, or the
  // menus' Ctrl chords, and nothing mid-composition. The hook itself does not
  // fit: it has no highlight on an empty query and always starts on the first
  // row, where this list keeps the current choice and passes over the pinned
  // Auto row to the best match.
  function onKeyDown(event: React.KeyboardEvent<HTMLInputElement>): void {
    if (event.nativeEvent.isComposing || event.keyCode === 229) return
    const chord =
      event.ctrlKey && !event.altKey && !event.metaKey && !event.shiftKey
    const plain =
      !event.ctrlKey && !event.altKey && !event.metaKey && !event.shiftKey
    const down =
      (plain && event.key === "ArrowDown") ||
      (chord && (event.key === "n" || event.key === "j"))
    const up =
      (plain && event.key === "ArrowUp") ||
      (chord && (event.key === "p" || event.key === "k"))
    if (down || up) {
      event.preventDefault()
      move(down ? 1 : -1)
    } else if (plain && event.key === "Enter") {
      event.preventDefault()
      if (active !== null) onPick(active.row)
    }
  }

  function keepFieldFocus(event: React.PointerEvent | React.MouseEvent): void {
    if (event.target instanceof HTMLInputElement) return
    event.preventDefault()
  }

  function renderRow(
    key: RowKey,
    icon: React.ReactNode,
    label: React.ReactNode,
  ): React.ReactNode {
    const checked = key === AUTO ? current === undefined : current === key
    const selected = active !== null && active.row === key
    return (
      <div
        key={key ?? "auto"}
        id={rowId(key)}
        role="option"
        aria-selected={selected}
        aria-checked={checked}
        // "true", not mere presence: the theme's `data-selected:` variant
        // matches [data-selected=true].
        data-selected={selected}
        data-checked={checked}
        onPointerMove={() => {
          if (!selected) setHighlight({ row: key, reveal: false })
        }}
        onClick={() => onPick(key)}
        // The searchable lists' row look (ui/command.tsx's CommandItem). A
        // 44px floor on a phone and for a finger at any width; only a pointer
        // that is not coarse at desktop width gets the compact row.
        className="relative flex min-h-11 cursor-pointer items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-hidden select-none data-selected:bg-muted data-selected:text-foreground md:not-pointer-coarse:min-h-0 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4"
      >
        {icon}
        {label}
        <CheckIcon className={cn("ml-auto", !checked && "opacity-0")} />
      </div>
    )
  }

  return (
    // min-h-0 lets the column shrink when the popup hits its viewport cap, so
    // the list scrolls instead of pushing the field off screen. The field and
    // the list wear the searchable lists' look (ui/command.tsx).
    <div
      className="flex min-h-0 flex-col overflow-hidden rounded-xl bg-popover p-1 text-popover-foreground"
      // A press anywhere but the field (a row, the separator, the padding,
      // the no-match sentence) keeps focus in the field, which owns the
      // keyboard; a row still picks on its click.
      onPointerDown={keepFieldFocus}
      onMouseDown={keepFieldFocus}
    >
      <div className="p-1 pb-0">
        {/* 40px for a finger and on a phone, the touch floor; the compact
            32px only for a fine pointer at desktop width. */}
        <InputGroup className="h-8! max-md:h-10! pointer-coarse:h-10! rounded-lg! border-input/30 bg-input/30 shadow-none! *:data-[slot=input-group-addon]:pl-2!">
          <input
            ref={fieldRef}
            role="combobox"
            aria-label="Search languages"
            aria-expanded="true"
            aria-controls={listId}
            aria-autocomplete="list"
            aria-activedescendant={activeId}
            autoComplete="off"
            autoCorrect="off"
            spellCheck={false}
            placeholder="Search languages…"
            // Focused on open on every pointer and layout, a phone included,
            // where it brings the soft keyboard up at once.
            autoFocus
            value={search}
            onChange={(event) => changeSearch(event.target.value)}
            onKeyDown={onKeyDown}
            // text-base on phones so iOS Safari does not zoom the page when
            // the field focuses (the Input primitive's convention).
            // Fills the box's height, so a tap on its top or bottom band
            // still lands in the field.
            className="h-full w-full self-stretch bg-transparent text-base outline-hidden md:text-sm"
          />
          <InputGroupAddon>
            <SearchIcon className="size-4 shrink-0 opacity-50" />
          </InputGroupAddon>
        </InputGroup>
      </div>
      <div
        ref={scrollerRef}
        className="no-scrollbar min-h-0 scroll-py-1 overflow-x-hidden overflow-y-auto"
      >
        <div id={listId} role="listbox" aria-label="Languages" className="p-1">
          {renderRow(AUTO, <Wand2 />, "Auto")}
          <div aria-hidden="true" className="-mx-1 my-1 h-px bg-border" />
          {matches.map((choice) =>
            renderRow(
              choice.id,
              <Code2 />,
              <span>
                <MatchHighlight
                  text={choice.label}
                  ranges={matchCharRanges(choice.label, [needle])}
                />
              </span>,
            ),
          )}
        </div>
        {/* Always mounted, so a screen reader announces the sentence when it
            appears rather than missing a region that arrived with it. */}
        <p
          role="status"
          className={cn(
            "text-center text-sm text-muted-foreground",
            matches.length === 0 && "px-3 py-6",
          )}
        >
          {matches.length === 0 ? "No language matches that search." : ""}
        </p>
      </div>
    </div>
  )
}
