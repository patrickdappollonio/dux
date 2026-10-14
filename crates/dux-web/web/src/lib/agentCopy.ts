// What happens to text a program in a terminal pane copies (OSC 52) on its way
// to the browser clipboard, and what the user is offered when the browser says
// no.
//
// The browser only lets a page write the clipboard for a few seconds after the
// user's last key press or click, so a copy the program sends later than that is
// refused, and over plain HTTP the modern clipboard API does not exist at all.
// The program already believes the text was copied, so a refusal must not lose
// it: the user gets a toast holding the text, with a button whose press is a
// fresh gesture the browser will accept.

import { useSyncExternalStore } from "react"

import { copyToClipboard } from "./clipboard"
import { notifyError, notifySuccess, notifyWarning } from "./notify"
import { chip, type Prose, prose } from "./prose"

/// How much of the copied text the toast quotes, in characters.
export const COPY_PREVIEW_CHARS = 60

/// The copied text squeezed onto one short line for the toast: whitespace runs
/// become one space, and anything past the cap becomes an ellipsis. Counts
/// code points, so a cut never lands inside a character.
export function copyPreview(text: string): string {
  const flat = Array.from(text.replace(/\s+/g, " ").trim())
  if (flat.length <= COPY_PREVIEW_CHARS) return flat.join("")
  return `${flat.slice(0, COPY_PREVIEW_CHARS).join("")}…`
}

/// Copy without taking focus from the user.
///
/// A program's copy arrives while the user is typing, into the terminal or into
/// the message box, so the hidden-textarea fallback hands focus back to that
/// element: a textarea keeps its caret and selection across the blur, so the
/// draft is exactly as it was. The modern API is awaited and may finish after the
/// user has moved on, so its copy leaves focus alone.
export function copyKeepingFocus(text: string): Promise<boolean> {
  return copyToClipboard(text, { keepFocus: true })
}

/// The toast id for a pane's blocked copies: a newer one from the same pane
/// replaces the older, since the program's clipboard only ever holds the last.
export function blockedCopyToastId(paneTag: string): string {
  return `dux-copy-blocked-${paneTag}`
}

function blockedMessage(source: string, text: string): Prose {
  const preview = copyPreview(text)
  return preview
    ? prose`${chip(source)} tried to copy ${chip(preview)} to your clipboard, but the browser blocked it. Press Copy to copy it now.`
    : prose`${chip(source)} tried to copy some blank text to your clipboard, but the browser blocked it. Press Copy to copy it now.`
}

function refusedMessage(source: string): Prose {
  return prose`Your browser refused to copy the text from ${chip(source)}, even from a button press. Press Show text to select it and copy it yourself.`
}

/// Raise the toast for a copy the browser refused. The toast's button closure is
/// the only place the full text lives, so it lasts until a copy succeeds or the
/// toast is dismissed.
export function reportBlockedCopy(paneTag: string, source: string, text: string): void {
  const id = blockedCopyToastId(paneTag)
  // Sticky: the program already counts the text as copied, so once this toast
  // goes the text is somewhere only a copy from inside the program can reach.
  notifyWarning(blockedMessage(source, text), {
    id,
    sticky: true,
    action: { label: "Copy", onClick: () => void retryBlockedCopy(id, source, text) },
  })
}

async function retryBlockedCopy(id: string, source: string, text: string): Promise<void> {
  if (await copyKeepingFocus(text)) {
    notifySuccess(prose`Copied the text from ${chip(source)} to your clipboard.`, { id })
    return
  }
  // Sticky for the same reason as the toast it replaces.
  notifyError(refusedMessage(source), {
    id,
    sticky: true,
    action: { label: "Show text", onClick: () => showCopyText({ source, text }) },
  })
}

/// What the show-text dialog is showing, if anything.
export interface CopyTextView {
  source: string
  text: string
}

let shown: CopyTextView | null = null
const listeners = new Set<() => void>()

function publish(next: CopyTextView | null): void {
  shown = next
  for (const l of [...listeners]) l()
}

export function showCopyText(view: CopyTextView): void {
  publish(view)
}

export function closeCopyText(): void {
  publish(null)
}

function subscribe(l: () => void): () => void {
  listeners.add(l)
  return () => listeners.delete(l)
}

export function shownCopyText(): CopyTextView | null {
  return shown
}

export function useCopyText(): CopyTextView | null {
  return useSyncExternalStore(subscribe, shownCopyText, shownCopyText)
}
