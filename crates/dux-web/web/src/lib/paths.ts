// Small path helpers shared by the folder pickers.
//
// Its own module because a file of React components may export only components
// (the fast-refresh rule), and both folder pickers plus the browse list itself
// need the same trailing-segment rule. Two copies of it would drift the moment
// one of them learned about trailing slashes and the other did not.

/**
 * The trailing segment of a path: the folder's own name.
 *
 * Tolerates a trailing slash (a directory path may or may not carry one) and
 * answers the path itself for the filesystem root, so a caller never has to
 * special-case either.
 */
export function baseName(path: string): string {
  const trimmed = path.endsWith("/") && path !== "/" ? path.slice(0, -1) : path
  const idx = trimmed.lastIndexOf("/")
  return idx >= 0 ? trimmed.slice(idx + 1) || trimmed : trimmed
}
