/**
 * The hover-revealed `⋯` wrapper's escape hatch for a finger.
 *
 * A row's actions menu consumes no layout space at rest and animates open on
 * hover, focus-within, or while its menu is open. A coarse pointer has none of
 * those at rest: there is no hover, and nothing is focused until something is
 * pressed, so a trigger that only appears on hover is a control a finger can
 * never reach. The tenet is therefore "always shows on touch", and this is the
 * one place that says what that means in CSS.
 *
 * It is a POINTER question, not a width one. Every one of these wrappers used
 * to answer it with `max-md:`, which is the viewport-width breakpoint: a tablet
 * in landscape gets the desktop layout with a finger for a pointer, and the
 * trigger vanished there with no way to bring it back. The width classes stay
 * where they are (they are what keeps the phone's own layout unchanged); this
 * rides on top of them.
 *
 * These are unprefixed `pointer-coarse:` utilities on purpose. Tailwind emits
 * the `(pointer: coarse)` block after the `min-width`/`max-width` ones and
 * after the bare utilities, so this wins over every resting-state and
 * hover-state class in the wrapper without any of them having to change, which
 * is what keeps a mouse's behavior byte-identical.
 */
export const ALWAYS_REVEALED_ON_TOUCH =
  "pointer-coarse:max-w-none pointer-coarse:opacity-100"
