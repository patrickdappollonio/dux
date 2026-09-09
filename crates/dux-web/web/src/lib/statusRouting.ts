// Which engine statuses a surface renders. Pure and DOM-free so the rule is
// unit-testable on its own; the store wires it around its single status-toast
// call site.

/**
 * Whether a `status` frame should raise a toast in this tab. The workspace tab
 * renders everything, since the server has already scope-filtered; the standalone
 * editor tab renders only statuses addressed to its own connection.
 *
 * Anything other than the literal scope `all` counts as addressed, so a new addressed
 * form needs no client change. A missing scope reads as a broadcast, which keeps the
 * editor tab quiet.
 */
export function statusToastAllowed(
  scope: unknown,
  standaloneEditor: boolean,
): boolean {
  if (!standaloneEditor) return true
  if (scope === undefined || scope === null) return false
  return scope !== "all"
}
