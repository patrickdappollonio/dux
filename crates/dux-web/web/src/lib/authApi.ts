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
  blockedWhere,
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
  }
}

export type StatusAnswer =
  | { kind: "status"; status: AuthStatus }
  | { kind: "blocked"; where: string | null }
  | { kind: "broken"; detail: string }
  /** The server answered, but not with the document (an older server). */
  | { kind: "unknown" }
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
    const where = blockedWhere(resp.status, body)
    if (where !== undefined) return { kind: "blocked", where }
    const detail = brokenDetail(resp.status, body)
    if (detail !== undefined) return { kind: "broken", detail }
    return { kind: "unknown" }
  }
  try {
    return { kind: "status", status: normalizeAuthStatus(await resp.json()) }
  } catch {
    return { kind: "unknown" }
  }
}

export type LoginAnswer =
  | { kind: "ok" }
  | { kind: "wrong" }
  | { kind: "rate_limited"; retryAfterSeconds: number | null; from: string | null }
  | { kind: "blocked"; where: string | null }
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
  const where = blockedWhere(resp.status, body)
  if (where !== undefined) return { kind: "blocked", where }
  const detail = brokenDetail(resp.status, body)
  if (detail !== undefined) return { kind: "broken", detail }
  return { kind: "refused", message: refusalSentence(resp.status, body, "sign in") }
}
