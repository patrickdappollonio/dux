// Small path helpers shared by the folder pickers and the browse list. Its own module because
// a file of React components may export only components (the fast-refresh rule).

/**
 * The trailing segment of a path: the folder's own name. Tolerates a trailing slash and
 * answers the path itself for the filesystem root, so a caller special-cases neither.
 */
export function baseName(path: string): string {
  const trimmed = path.endsWith("/") && path !== "/" ? path.slice(0, -1) : path
  const idx = trimmed.lastIndexOf("/")
  return idx >= 0 ? trimmed.slice(idx + 1) || trimmed : trimmed
}
