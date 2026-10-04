// The PROTECTED auth routes: ending this browser's session, changing the
// password, and dismissing the no-password warning for good. They go through
// the one fetch door (`apiFetch`) like every other protected request, so a
// session that ended reaches the gate however it is met, and each is bounded by
// the browser's request deadline. Refusals come back as sentences
// (`authErrors.ts`), never as the body the server sent.

import { apiFetch, isAuthInterruption } from "./apiFetch"
import { count, readErrorBody, refusalSentence } from "./authErrors"
import { getConnectionId } from "./connection"
import { reconnectAttemptTimeoutMs } from "./connectionTiming"
import { isDeadline, withDeadline } from "./deadline"

function connectionIdHeader(): Record<string, string> {
  const id = getConnectionId()
  return id ? { "x-connection-id": id } : {}
}

function post(path: string, body?: unknown): Promise<Response> {
  return withDeadline(reconnectAttemptTimeoutMs(), (signal) =>
    apiFetch(path, {
      method: "POST",
      credentials: "same-origin",
      headers: {
        ...connectionIdHeader(),
        ...(body === undefined ? {} : { "content-type": "application/json" }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal,
    }),
  )
}

export type LogoutAnswer =
  | { kind: "ok" }
  /** The gate has taken over: the address is blocked, the config is broken,
   * or the page was already on a gate page. */
  | { kind: "gate" }
  | { kind: "unreachable"; timedOut: boolean }
  | { kind: "refused"; message: string }

export async function postLogout(): Promise<LogoutAnswer> {
  let resp: Response
  try {
    resp = await post("/api/v1/auth/logout")
  } catch (e) {
    // No session to end is the outcome a sign-out wanted. Anything else the
    // door met (a block, a broken config, a page already on a gate page) is
    // the gate's to show.
    if (isAuthInterruption(e)) return e.refusal === "signed_out" ? { kind: "ok" } : { kind: "gate" }
    return { kind: "unreachable", timedOut: isDeadline(e) }
  }
  if (resp.ok) return { kind: "ok" }
  const body = await readErrorBody(resp)
  return { kind: "refused", message: refusalSentence(resp.status, body, "sign out") }
}

export type PasswordAnswer =
  | { kind: "ok" }
  | { kind: "signed_out" }
  | { kind: "unreachable"; timedOut: boolean }
  /** `score` is the server's 0-4 strength result when it sent one. */
  | { kind: "refused"; message: string; score: number | null }

export async function postPassword(change: {
  current?: string
  next: string
}): Promise<PasswordAnswer> {
  const body: Record<string, string> = { new: change.next }
  if (change.current !== undefined) body.current = change.current
  let resp: Response
  try {
    resp = await post("/api/v1/auth/password", body)
  } catch (e) {
    if (isAuthInterruption(e)) return { kind: "signed_out" }
    return { kind: "unreachable", timedOut: isDeadline(e) }
  }
  if (resp.ok) return { kind: "ok" }
  const err = await readErrorBody(resp)
  return {
    kind: "refused",
    message: refusalSentence(
      resp.status,
      err,
      "change the password",
      resp.headers.get("retry-after"),
    ),
    score: count(err.json.score),
  }
}

export async function postDismissNoAuthWarning(): Promise<void> {
  let resp: Response
  try {
    resp = await post("/api/v1/auth/dismiss-no-auth-warning")
  } catch (e) {
    if (isAuthInterruption(e)) throw e
    throw new Error(
      isDeadline(e)
        ? "dux did not answer in time, so the warning will show again. Try once more."
        : "Could not reach dux, so the warning will show again. Try once more.",
      { cause: e },
    )
  }
  if (!resp.ok) {
    const body = await readErrorBody(resp)
    throw new Error(refusalSentence(resp.status, body, "save that choice"))
  }
}
