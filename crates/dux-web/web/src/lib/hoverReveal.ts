import { ALWAYS_REVEALED_ON_TOUCH } from "./touchReveal"

/** The row or pill whose hover and keyboard focus reveal its `⋯` wrapper. */
export const REVEAL_ROW = "group/reveal"

/**
 * The wrapper around a hover-revealed `⋯`, opened to the site's `--reveal-cap`
 * on hover, an open menu, keyboard focus in its row, and always under a coarse
 * pointer; its clip lifts while the `⋯` holds keyboard focus.
 */
export const HOVER_REVEAL = [
  "flex shrink-0 items-center overflow-hidden transition-[max-width,opacity] duration-200 ease-out motion-reduce:transition-none",
  "max-w-0 opacity-0",
  "group-hover/reveal:max-w-(--reveal-cap) group-hover/reveal:opacity-100",
  "group-focus-visible/reveal:max-w-(--reveal-cap) group-focus-visible/reveal:opacity-100",
  "group-has-[:focus-visible]/reveal:max-w-(--reveal-cap) group-has-[:focus-visible]/reveal:opacity-100",
  "has-[[data-popup-open]]:max-w-(--reveal-cap) has-[[data-popup-open]]:opacity-100",
  "has-[:focus-visible]:overflow-visible pointer-coarse:overflow-visible",
  ALWAYS_REVEALED_ON_TOUCH,
].join(" ")

/** Also reveals on mouse focus inside the row, for the rows that always have. */
export const REVEALED_ON_ANY_FOCUS =
  "group-focus-within/reveal:max-w-(--reveal-cap) group-focus-within/reveal:opacity-100"

/** Holds the slot open and painted at phone width, whatever the pointer. */
export const ALWAYS_REVEALED_ON_PHONE = "max-md:max-w-none max-md:opacity-100"
