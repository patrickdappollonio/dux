import type { ModalKind, ModalSize } from "./types"

export type ModalPresentation = "card" | "cover"

/** A confirm is a short question and stays a card; every other kind takes the
 * whole of a phone's screen, so nothing shows behind it. */
export function modalPresentation(kind: ModalKind, phone: boolean): ModalPresentation {
  return phone && kind !== "confirm" ? "cover" : "card"
}

// Spelled out in full so Tailwind sees each class.
export const CARD_WIDTH: Record<ModalSize, string> = {
  sm: "sm:max-w-sm",
  md: "sm:max-w-md",
  lg: "sm:max-w-lg",
  xl: "sm:max-w-xl",
  "2xl": "sm:max-w-2xl",
  "3xl": "sm:max-w-3xl",
  "4xl": "sm:max-w-4xl",
}

/** The space under a cover's pinned footer, and under its body when it has no
 * footer: the home indicator's inset, or 1rem where there is none. */
export const COVER_BOTTOM_INSET = "pb-[max(env(safe-area-inset-bottom),1rem)]"
