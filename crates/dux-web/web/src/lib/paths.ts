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

/**
 * The name a standalone agent gets when the user types none: the twin of
 * dux-core's `git::standalone_agent_title` with an empty typed name, pinned by
 * shared vectors.
 *
 * It exists because the create dialog PROMISES this name in its placeholder
 * ("defaults to ..."), and a promise the server then does not keep is worse
 * than no promise at all. The server's rules are gentle on purpose, because the
 * result is a label rather than a path or a ref: collapse runs of whitespace,
 * trim, and when nothing usable is left (the filesystem root, a name of only
 * whitespace) fall back to a fixed word instead of an empty string.
 */
export function standaloneAgentDefaultName(folderPath: string): string {
  const collapsed = baseName(folderPath).split(/\s+/).filter(Boolean).join(" ")
  // `baseName` answers "/" for the root, which is not a name either.
  return collapsed === "" || collapsed === "/" ? "Standalone agent" : collapsed
}
