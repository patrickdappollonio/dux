/** Product fallback used when no instance title is configured (or it is blank). */
export const DEFAULT_INSTANCE_TITLE = "dux"

/**
 * Resolve the operator-configured instance title (`config.server.title`, carried
 * on the bootstrap document) into the string the UI should display. Collapses any
 * internal run of CR, LF, or tab characters to a single space and trims
 * surrounding whitespace, then falls back to {@link DEFAULT_INSTANCE_TITLE} when
 * the value is missing, empty, or whitespace-only. Used for both the browser tab
 * title and the brand wordmark so the two surfaces never drift (browsers truncate
 * a tab title at a newline and render a tab as a space, so collapsing these keeps
 * the tab and the wordmark identical).
 */
export function resolveInstanceTitle(raw: string | null | undefined): string {
  const normalized = (raw ?? "").replace(/[\r\n\t]+/g, " ").trim()
  return normalized === "" ? DEFAULT_INSTANCE_TITLE : normalized
}

/**
 * The browser-tab title for the current surface. The workspace tab is simply
 * the instance title; the standalone editor tab prefixes "Editor" so the two
 * tabs can be told apart in a tab strip full of dux instances. The separator
 * is an em dash by the maintainer's explicit choice for this one string.
 * The attention-count prefix (formatTabTitle) wraps OUTSIDE this, so a
 * flagged agent still surfaces in the editor tab's title.
 */
export function pageTitle(base: string, standaloneEditor: boolean): string {
  return standaloneEditor ? `Editor — ${base}` : base
}
