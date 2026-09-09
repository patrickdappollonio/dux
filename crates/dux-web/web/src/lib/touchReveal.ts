/**
 * The hover-revealed `⋯` wrapper's escape hatch for a finger: a coarse pointer has
 * no hover and nothing focused at rest, so a hover-only trigger is unreachable.
 *
 * A pointer question, never a width one: a tablet in landscape gets the desktop
 * layout with a finger for a pointer. The classes are unprefixed
 * `pointer-coarse:` utilities, which Tailwind emits after the width and bare ones,
 * so they win over the wrapper's resting and hover classes without changing them.
 */
export const ALWAYS_REVEALED_ON_TOUCH =
  "pointer-coarse:max-w-none pointer-coarse:opacity-100"
