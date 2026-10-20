// The two PUBLIC auth routes: the status read and the login. Deliberately plain
// `fetch` rather than `apiFetch`: they must work while the page is signed out,
// and a login's 401 means "wrong password", not "the session ended". Every
// protected auth route (logout, password, the warning's dismissal) lives in
// `authActions.ts` and goes through the door like any other request.
//
// Both are bounded by the browser's request deadline (the configured
// reconnect-attempt timeout), so a server that never answers cannot leave the
// page waiting on a blank screen.

import { apiUrl } from "./apiBase"
import {
  isBlocked,
  brokenDetail,
  count,
  limitedFrom,
  readErrorBody,
  record,
  refusalSentence,
  retryAfterSeconds,
  str,
} from "./authErrors"
import { reconnectAttemptTimeoutMs } from "./connectionTiming"
import { isDeadline, withDeadline } from "./deadline"

export { retryAfterSeconds }

/// `GET /api/v1/auth/status`, normalized. A missing flag reads as false, so a
/// partial answer never invents a login; the transport stays unknown (null)
/// rather than being called unencrypted, and the minimums stay null when the
/// server does not report them.
export interface AuthStatus {
  password_set: boolean
  required_here: boolean
  signed_in: boolean
  /** How the server classed this connection; shown, never decided on. */
  client_class: string | null
  transport_encrypted: boolean | null
  no_auth_warning: boolean
  weak_password: boolean
  can_set_first_password: boolean
  /** Null when auth is healthy; otherwise the server's description of the
   * problem, empty when it gave none (it gives one only to this machine and
   * the tailnet). */
  auth_broken: string | null
  minimum_password_length: number | null
  minimum_password_score: number | null
  /** Why this device, which reached dux over loopback, is treated as the
   * network (so it signs in, and cannot set the first password). One line,
   * setting names in backticks for the chip renderer. Null when it is not. */
  required_reason: string | null
  /** Which dux this is, kept across its restarts (`duxInstance.ts`). Null
   * from a dux too old to say. */
  instance: string | null
}

function flag(v: unknown): boolean {
  return v === true
}

export function normalizeAuthStatus(raw: unknown): AuthStatus {
  const r = record(raw)
  const broken = r.auth_broken
  return {
    password_set: flag(r.password_set),
    required_here: flag(r.required_here),
    signed_in: flag(r.signed_in),
    client_class: typeof r.client_class === "string" ? r.client_class : null,
    transport_encrypted:
      typeof r.transport_encrypted === "boolean" ? r.transport_encrypted : null,
    no_auth_warning: flag(r.no_auth_warning),
    weak_password: flag(r.weak_password),
    can_set_first_password: flag(r.can_set_first_password),
    auth_broken:
      typeof broken === "string" && broken !== ""
        ? broken
        : broken === true
          ? ""
          : null,
    minimum_password_length: count(r.minimum_password_length),
    minimum_password_score: count(r.minimum_password_score),
    required_reason: str(r.required_reason),
    instance: str(r.instance),
  }
}

// Whether an answer from the status route shows that something other than dux
// is listening. The route is public on every dux, so a page in its place, a
// bare 401 or a 404 is somebody else's. Only answers that a working dux never
// gives count: a gateway's 5xx while dux restarts behind it, a 408, a 429 or a
// 403 in words (dux's own Host check) stay unknown, because a flaky path to a
// real dux must never be called "not dux".
// The status document as every dux sends it: its three sign-in flags. The
// instance may be absent (a dux too old to have one); these never are.
function isStatusDocument(raw: unknown): boolean {
  const r = record(raw)
  return (
    typeof r.password_set === "boolean" &&
    typeof r.required_here === "boolean" &&
    typeof r.signed_in === "boolean"
  )
}

function foreignRefusal(status: number): boolean {
  return status === 401 || status === 404 || status === 405
}

export type StatusAnswer =
  | { kind: "status"; status: AuthStatus }
  | { kind: "blocked" }
  | { kind: "broken"; detail: string }
  /** The server answered, but not with the document (an older server). */
  | { kind: "unknown" }
  /** Something that is not dux answered (see `foreignRefusal`). */
  | { kind: "not_dux" }
  /** No answer at all: the network failed, or the deadline passed. */
  | { kind: "unreachable"; timedOut: boolean }

export async function fetchAuthStatus(): Promise<StatusAnswer> {
  let resp: Response
  try {
    resp = await withDeadline(reconnectAttemptTimeoutMs(), (signal) =>
      fetch(apiUrl("/api/v1/auth/status"), {
        credentials: "same-origin",
        cache: "no-store",
        signal,
      }),
    )
  } catch (e) {
    return { kind: "unreachable", timedOut: isDeadline(e) }
  }
  if (!resp.ok) {
    const body = await readErrorBody(resp)
    if (isBlocked(resp.status, body)) return { kind: "blocked" }
    const detail = brokenDetail(resp.status, body)
    if (detail !== undefined) return { kind: "broken", detail }
    // A JSON refusal with a code is dux's own, whatever its status.
    if (!foreignRefusal(resp.status) || str(body.json.error) !== null) return { kind: "unknown" }
    // Only an answer read whole is evidence; one cut mid-read stays unknown.
    try {
      await resp.text()
    } catch {
      return { kind: "unknown" }
    }
    return { kind: "not_dux" }
  }
  // Only a full answer whose body arrived and is not JSON counts as foreign (a
  // parse error); a connection cut mid-read, or another 2xx, stays unknown.
  if (resp.status !== 200) return { kind: "unknown" }
  let raw: unknown
  try {
    raw = await resp.json()
  } catch (e) {
    return e instanceof SyntaxError ? { kind: "not_dux" } : { kind: "unknown" }
  }
  if (!isStatusDocument(raw)) return { kind: "not_dux" }
  return { kind: "status", status: normalizeAuthStatus(raw) }
}

export type LoginAnswer =
  | { kind: "ok" }
  | { kind: "wrong" }
  | { kind: "rate_limited"; retryAfterSeconds: number | null; from: string | null }
  | { kind: "blocked" }
  | { kind: "broken"; detail: string }
  | { kind: "unreachable"; timedOut: boolean }
  | { kind: "refused"; message: string }

export async function postLogin(password: string): Promise<LoginAnswer> {
  let resp: Response
  try {
    resp = await withDeadline(reconnectAttemptTimeoutMs(), (signal) =>
      fetch(apiUrl("/api/v1/auth/login"), {
        method: "POST",
        credentials: "same-origin",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ password }),
        signal,
      }),
    )
  } catch (e) {
    return { kind: "unreachable", timedOut: isDeadline(e) }
  }
  if (resp.ok) return { kind: "ok" }
  const body = await readErrorBody(resp)
  if (resp.status === 401) return { kind: "wrong" }
  if (resp.status === 429) {
    return {
      kind: "rate_limited",
      retryAfterSeconds: retryAfterSeconds(
        resp.headers.get("retry-after"),
        body.json,
        Date.now(),
      ),
      from: limitedFrom(body.json),
    }
  }
  if (isBlocked(resp.status, body)) return { kind: "blocked" }
  const detail = brokenDetail(resp.status, body)
  if (detail !== undefined) return { kind: "broken", detail }
  return { kind: "refused", message: refusalSentence(resp.status, body, "sign in") }
}
