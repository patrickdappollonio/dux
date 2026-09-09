/** Product fallback used when no instance title is configured (or it is blank). */
export const DEFAULT_INSTANCE_TITLE = "dux"

/**
 * Resolve the operator-configured instance title (`config.server.title`) into the displayed
 * string: internal runs of CR, LF or tab collapse to one space, surrounding whitespace is
 * trimmed, and an empty result falls back to {@link DEFAULT_INSTANCE_TITLE}. Browsers truncate
 * a tab title at a newline, so collapsing keeps the tab title and the wordmark identical.
 */
export function resolveInstanceTitle(raw: string | null | undefined): string {
  const normalized = (raw ?? "").replace(/[\r\n\t]+/g, " ").trim()
  return normalized === "" ? DEFAULT_INSTANCE_TITLE : normalized
}

/**
 * The browser-tab title for the current surface: the standalone editor tab prefixes "Editor"
 * so two tabs can be told apart in a strip full of dux instances. The separator is an em dash
 * by the maintainer's explicit choice for this one string. `formatTabTitle`'s attention prefix
 * wraps outside this, and the editor tab's count is always zero.
 */
export function pageTitle(base: string, standaloneEditor: boolean): string {
  return standaloneEditor ? `Editor — ${base}` : base
}
