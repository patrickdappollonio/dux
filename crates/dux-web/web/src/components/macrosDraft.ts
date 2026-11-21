// The Macros dialog's working copy: every edit, addition, delete and reorder
// lands here, and only Save sends the list on.

import { macroMatchesSurface, type EditedMacro, type MacrosEdit } from "@/lib/macros"
import type { MacroSurface, MacroView } from "@/lib/types"

// `key` is stable for the dialog's life.
export interface DraftMacro extends EditedMacro {
  key: number
}

export type MacroFilter = "all" | "agent" | "terminal"

export function seedDraft(edit: MacrosEdit): DraftMacro[] {
  return edit.macros.map((macro, key) => ({ ...macro, key }))
}

// The editing session as it stands, to save or to set aside: names trimmed,
// as Save writes them, and the macro with `selectedKey` as the one on screen.
export function draftToEdit(
  items: DraftMacro[],
  baseline: MacroView[],
  version: string | null,
  selectedKey: number | null,
): MacrosEdit {
  const selected = items.findIndex((item) => item.key === selectedKey)
  return {
    macros: items.map(({ name, text, surface, origin }) => ({
      name: name.trim(),
      text,
      surface,
      origin,
    })),
    baseline,
    version,
    selected: selected === -1 ? null : selected,
  }
}

// Whether this row differs from what is saved: new, or any field changed. A
// name differing only in surrounding spaces is not a change, since Save trims it.
export function isModified(item: DraftMacro, baseline: MacroView[]): boolean {
  if (item.origin === null) return true
  const saved = baseline[item.origin]
  return (
    saved === undefined ||
    saved.name !== item.name.trim() ||
    saved.text !== item.text ||
    saved.surface !== item.surface
  )
}

// How many unsaved changes the footer reports: one per new or edited macro,
// one per deleted macro, and one for the order when saved macros moved.
export function changeCount(items: DraftMacro[], baseline: MacroView[]): number {
  let count = items.filter((item) => isModified(item, baseline)).length
  const kept = items.flatMap((item) => (item.origin === null ? [] : [item.origin]))
  count += baseline.length - new Set(kept).size
  if (kept.some((origin, i) => i > 0 && origin < kept[i - 1])) count += 1
  return count
}

// The list Save sends, in draft order, names trimmed.
export function draftToMacros(items: DraftMacro[]): MacroView[] {
  return items.map(({ name, text, surface }) => ({ name: name.trim(), text, surface }))
}

export function nextKey(items: DraftMacro[]): number {
  return items.reduce((max, item) => Math.max(max, item.key), -1) + 1
}

export function updateItem(
  items: DraftMacro[],
  key: number,
  patch: Partial<MacroView>,
): DraftMacro[] {
  return items.map((item) => (item.key === key ? { ...item, ...patch } : item))
}

export function removeItem(items: DraftMacro[], key: number): DraftMacro[] {
  return items.filter((item) => item.key !== key)
}

// Moves the row `activeKey` to where `overKey` is. Returns `items` itself for
// a drop that moves nothing, so a caller can tell.
export function moveItem(items: DraftMacro[], activeKey: number, overKey: number): DraftMacro[] {
  const from = items.findIndex((item) => item.key === activeKey)
  const to = items.findIndex((item) => item.key === overKey)
  if (from === -1 || to === -1 || from === to) return items
  const next = items.slice()
  const [moved] = next.splice(from, 1)
  next.splice(to, 0, moved)
  return next
}

export function moveToTop(items: DraftMacro[], key: number): DraftMacro[] {
  if (items.length === 0) return items
  return moveItem(items, key, items[0].key)
}

// A copy placed right after the original, under the first free "<name> copy",
// "<name> copy 2", … so the draft stays valid. The copy of an untitled macro
// is untitled too.
export function duplicateItem(items: DraftMacro[], key: number): DraftMacro[] {
  const index = items.findIndex((item) => item.key === key)
  if (index === -1) return items
  const source = items[index]
  let name = ""
  if (source.name.trim() !== "") {
    const names = new Set(items.map((item) => item.name.trim()))
    const base = `${source.name.trim()} copy`
    name = base
    for (let n = 2; names.has(name); n++) name = `${base} ${n}`
  }
  const copy: DraftMacro = { ...source, name, key: nextKey(items), origin: null }
  return [...items.slice(0, index + 1), copy, ...items.slice(index + 1)]
}

export function addItem(items: DraftMacro[], surface: MacroSurface): DraftMacro[] {
  return [...items, { name: "", text: "", surface, key: nextKey(items), origin: null }]
}

// The filter a macro is listed under: Agent and Terminal each include the
// macros shown on both, as the picker on that kind of pane would list them.
export function matchesFilter(surface: MacroSurface, filter: MacroFilter): boolean {
  return filter === "all" || macroMatchesSurface(surface, filter)
}

export function filterCounts(items: DraftMacro[]): Record<MacroFilter, number> {
  return {
    all: items.length,
    agent: items.filter((item) => matchesFilter(item.surface, "agent")).length,
    terminal: items.filter((item) => matchesFilter(item.surface, "terminal")).length,
  }
}

// The search, normalized as the highlight's needle: trimmed and lowercased.
export function searchNeedle(query: string): string {
  return query.trim().toLowerCase()
}

export function matchesSearch(item: MacroView, needle: string): boolean {
  if (needle === "") return true
  return item.name.toLowerCase().includes(needle) || item.text.toLowerCase().includes(needle)
}
