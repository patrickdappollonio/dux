import type { ModalAnchor, ModalHeight, ModalKind, ModalSize } from "./types"

export type ModalPresentation = "card" | "cover"

/** A confirm is a short question and stays a card; every other kind takes the
 * whole of a phone's screen, so nothing shows behind it. */
export function modalPresentation(kind: ModalKind, phone: boolean): ModalPresentation {
  return phone && kind !== "confirm" ? "cover" : "card"
}

// Spelled out in full so Tailwind sees each class. From 640px on, a width
// that could meet the window's edge keeps the card's 1rem margin instead.
export const CARD_WIDTH: Record<ModalSize, string> = {
  sm: "sm:max-w-sm",
  md: "sm:max-w-md",
  lg: "sm:max-w-lg",
  xl: "sm:max-w-xl",
  w600: "sm:max-w-[600px]",
  w640: "sm:max-w-[min(640px,calc(100%-2rem))]",
  w700: "sm:max-w-[min(700px,calc(100%-2rem))]",
  w704: "sm:max-w-[min(44rem,calc(100%-2rem))]",
  "3xl": "sm:max-w-[min(48rem,calc(100%-2rem))]",
  w880: "sm:max-w-[min(55rem,calc(100%-2rem))]",
  "4xl": "sm:max-w-[min(56rem,calc(100%-2rem))]",
  w940: "sm:max-w-[min(58.75rem,calc(100%-2rem))]",
}

export const CARD_HEIGHT: Record<ModalHeight, string> = {
  content: "",
  h640: "h-[min(40rem,calc(100dvh-2rem))]",
  h680: "h-[min(42.5rem,calc(100dvh-2rem))]",
}

// Hung from the top, the card's height cap leaves the same 1rem under it.
export const CARD_ANCHOR: Record<ModalAnchor, string> = {
  center: "",
  top: "top-[max(1rem,10dvh)] max-h-[calc(100dvh-max(1rem,10dvh)-1rem)] translate-y-0",
}

/** The space under a cover's pinned footer, and under its body when it has no
 * footer: the home indicator's inset, or 1rem where there is none. */
export const COVER_BOTTOM_INSET = "pb-[max(env(safe-area-inset-bottom),1rem)]"

/** How a button that stays focusable while it takes no press looks. */
export const INERT_BUTTON = "aria-disabled:pointer-events-none aria-disabled:opacity-50"
