import type { ModalKind } from "./types"

const FIELD =
  'input:not([type="hidden"], [type="checkbox"], [type="radio"], [type="button"], [type="submit"]):not(:disabled), textarea:not(:disabled), select:not(:disabled), [role="combobox"]:not([aria-disabled="true"]), [contenteditable="true"]'

const TABBABLE =
  'button:not(:disabled), a[href], input:not([type="hidden"]):not(:disabled), textarea:not(:disabled), select:not(:disabled), [tabindex]:not([tabindex="-1"])'

const TEXT_INPUT_TYPES = new Set(["text", "search", "url", "tel", "email", "password", "number"])

/** Whether focusing `el` raises a soft keyboard: something text is typed into.
 * A read-only field is not, so a report may name one to hold its selection. */
export function isTextEntry(el: Element | null): boolean {
  if (el instanceof HTMLTextAreaElement) return !el.readOnly && !el.disabled
  if (el instanceof HTMLInputElement) {
    return TEXT_INPUT_TYPES.has(el.type) && !el.readOnly && !el.disabled
  }
  return el instanceof HTMLElement && el.isContentEditable
}

export interface FocusParts {
  kind: ModalKind
  /** The pointer is a finger, so a focused text field would raise the keyboard. */
  coarse: boolean
  popup: HTMLElement
  /** Null when the modal has no body. */
  body: HTMLElement | null
  /** The caller's `initialFocus`, when it names something on screen. */
  named: HTMLElement | null
  /** The confirm's or the form's Cancel, or a card report's Close. */
  safeButton: HTMLElement | null
  search: HTMLElement | null
}

/** What takes focus when a modal opens or changes step, or null to leave
 * focus where a control inside the modal already took it. The pointer decides
 * whether a text field may be it; the layout plays no part. */
export function focusTarget(parts: FocusParts): HTMLElement | null {
  const { kind, coarse, popup, body, named, safeButton, search } = parts
  if (kind === "confirm") return safeButton ?? popup
  const active = document.activeElement
  const inside = active instanceof HTMLElement && active !== popup && popup.contains(active)
  if (coarse) {
    if (named !== null && !isTextEntry(named)) return named
    return inside && !isTextEntry(active) ? null : popup
  }
  if (named !== null) return named
  const byKind =
    kind === "form"
      ? (body?.querySelector<HTMLElement>(FIELD) ?? null)
      : kind === "picker"
        ? search
        : safeButton
  if (byKind !== null) return byKind
  if (inside) return null
  return body?.querySelector<HTMLElement>(TABBABLE) ?? popup
}
