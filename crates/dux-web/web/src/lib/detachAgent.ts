// Detaching an agent: the pure half. What the confirmation says, and how long
// it says dux will wait.
//
// The wait is configurable, so the number is read from the bootstrap document
// at render time rather than written into the copy. The wording is kept here,
// beside that read, so the sentence and the number it quotes cannot come apart.

import {
  DEFAULT_SHUTDOWN_TIMEOUT_SECONDS,
  type Bootstrap,
} from "./bootstrapApi"

/** How long dux waits for an agent to exit before forcing it, in seconds.
 * The server clamps and projects it; an older server that omits it falls back
 * to the documented default. */
export function shutdownGraceSeconds(
  bootstrap: Pick<Bootstrap, "shutdown_timeout_seconds"> | null | undefined,
): number {
  const value = bootstrap?.shutdown_timeout_seconds
  return typeof value === "number" && Number.isFinite(value) && value >= 0
    ? value
    : DEFAULT_SHUTDOWN_TIMEOUT_SECONDS
}

/** The body of the detach confirmation. Mirrors
 * `dux_core::engine::detach_confirm_body`, word for word, so the browser and
 * the terminal UI promise the same thing. */
export function detachConfirmBody(label: string, graceSeconds: number): string {
  return (
    `dux will ask "${label}" to shut down and wait up to ${graceSeconds} seconds ` +
    `for it to exit before forcing it. The agent stays in the list as Detached, ` +
    `and you can resume it later. Anything the agent is doing right now is ` +
    `interrupted.`
  )
}

/** Whether the agent has anything to detach: a detach asks a PROCESS to go, so
 * with none running there is nothing to ask. The menu item is absent rather
 * than disabled in that case, because a disabled row promises an action that is
 * not waiting on the user. */
export function agentHasLiveProcess(session: {
  tabs: { has_live_process: boolean }[]
}): boolean {
  return session.tabs.some((tab) => tab.has_live_process)
}
