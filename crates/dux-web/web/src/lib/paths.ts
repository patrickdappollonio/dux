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

/**
 * The name a standalone agent gets when the user types none: the twin of dux-core's
 * `git::standalone_agent_title` with an empty typed name, pinned by shared vectors, because
 * the create dialog promises this name in its placeholder. The rules are gentle, the result
 * being a label rather than a path or a ref: collapse runs of whitespace, trim, and fall back
 * to a fixed word when nothing usable is left.
 */
export function standaloneAgentDefaultName(folderPath: string): string {
  const collapsed = baseName(folderPath).split(/\s+/).filter(Boolean).join(" ")
  // `baseName` answers "/" for the root, which is not a name either.
  return collapsed === "" || collapsed === "/" ? "Standalone agent" : collapsed
}
