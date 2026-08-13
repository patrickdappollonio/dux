// Which engine statuses a surface renders. Pure and DOM-free so the rule is
// unit-testable on its own; the store wires it around its single status-toast
// call site.

/**
 * Whether a `status` frame should raise a toast in this tab.
 *
 * The workspace tab renders everything: the server has already scope-filtered,
 * so anything that arrives is meant for this client.
 *
 * The standalone editor tab is deliberately quiet. It renders only statuses
 * addressed to its own connection, never the workspace-wide broadcasts, so an
 * agent's progress and warnings stay in the workspace tab where they can be
 * acted on. Everything the editor raises itself is a local notification and
 * never travels through here, so it is unaffected.
 *
 * The wire scope is `"all"` for a broadcast and an object (`{connection: id}`)
 * for a connection-addressed status, so the rule is "anything other than the
 * literal string `all` is addressed". That shape survives the server growing a
 * new addressed form without a client change. A missing scope means a
 * malformed or older frame, and is read as a broadcast: silence is the
 * conservative answer for the editor tab, whose whole point is not to be
 * interrupted by workspace noise.
 */
export function statusToastAllowed(
  scope: unknown,
  standaloneEditor: boolean,
): boolean {
  if (!standaloneEditor) return true
  if (scope === undefined || scope === null) return false
  return scope !== "all"
}
