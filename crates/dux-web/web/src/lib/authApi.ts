// The wire half of the web login: the public auth routes and how their answers
// read. Deliberately plain `fetch` rather than `apiFetch`: these are the routes
// that must keep working while the page is signed out, and their 401s mean
// "wrong password", not "the session ended". `authGate.ts` owns what the
// answers do to the page.

import { apiUrl } from "./apiBase"

/// `GET /api/v1/auth/status`, normalized. A missing flag reads as false, so a
/// partial answer never invents a login; the transport stays unknown (null)
/// rather than being called unencrypted, and the minimums stay null when the
/// server does not report them.
export interface AuthStatus {
  password_set: boolean
  required_here: boolean
  signed_in: boolean
  /** How the server classed this connection ("this machine", "tailnet",
   * "network", "internet"); shown, never decided on. */
  client_class: string | null
  transport_encrypted: boolean | null
  no_auth_warning: boolean
  weak_password: boolean
  can_set_first_password: boolean
  /** Null when auth is healthy; otherwise the server's description of the
   * problem, empty when it gave none. */
  auth_broken: string | null
  minimum_password_length: number | null
  minimum_password_score: number | null
}

function record(raw: unknown): Record<string, unknown> {
  return typeof raw === "object" && raw !== null ? (raw as Record<string, unknown>) : {}
}

function flag(v: unknown): boolean {
  return v === true
}

function count(v: unknown): number | null {
  return typeof v === "number" && Number.isFinite(v) ? v : null
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
  }
}

/// A refusal body as the contract shapes it, plus the raw text for anything
/// that is not JSON (an older route, a proxy's page).
export interface ErrorBody {
  json: Record<string, unknown>
  text: string
}

/// Read a body without consuming the response the caller may still read.
export async function readErrorBody(resp: Response): Promise<ErrorBody> {
  const source = typeof resp.clone === "function" ? resp.clone() : resp
  const text = await source.text().then(
    (t) => t.trim(),
    () => "",
  )
  return { json: parseRecord(text), text }
}

function parseRecord(text: string): Record<string, unknown> {
  try {
    return record(JSON.parse(text))
  } catch {
    return {}
  }
}

function str(v: unknown): string | null {
  return typeof v === "string" && v !== "" ? v : null
}

/// Whether a refusal is the contract's "this browser has no valid session".
/// A 401 with no readable reason counts: no other dux route answers 401.
export function isAuthRequired(status: number, body: ErrorBody): boolean {
  if (status !== 401) return false
  const error = str(body.json.error)
  return error === null || error === "auth_required"
}

export function blockedWhere(status: number, body: ErrorBody): string | null | undefined {
  if (status !== 403 || body.json.error !== "blocked") return undefined
  return str(body.json.where)
}

export function brokenDetail(status: number, body: ErrorBody): string | undefined {
  if (status !== 503 || body.json.error !== "auth_config_invalid") return undefined
  return str(body.json.detail) ?? ""
}

export type StatusAnswer =
  | { kind: "status"; status: AuthStatus }
  | { kind: "blocked"; where: string | null }
  | { kind: "broken"; detail: string }
  | { kind: "unknown" }

export async function fetchAuthStatus(): Promise<StatusAnswer> {
  let resp: Response
  try {
    resp = await fetch(apiUrl("/api/v1/auth/status"), {
      credentials: "same-origin",
      cache: "no-store",
    })
  } catch {
    return { kind: "unknown" }
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

/// The wait a 429 asks for, in whole seconds: the `Retry-After` header (delta
/// seconds or an HTTP date), else a `retry_after`/`retry_after_seconds` body
/// field, else null.
export function retryAfterSeconds(
  header: string | null,
  body: Record<string, unknown> | null,
  now: number,
): number | null {
  if (header !== null && header.trim() !== "") {
    const h = header.trim()
    if (/^-?\d+$/.test(h)) return Math.max(0, Number(h))
    const at = Date.parse(h)
    if (!Number.isNaN(at)) return Math.max(0, Math.ceil((at - now) / 1000))
    return null
  }
  const b = body ?? {}
  const v = count(b.retry_after) ?? count(b.retry_after_seconds)
  return v === null ? null : Math.max(0, Math.ceil(v))
}

export type LoginAnswer =
  | { kind: "ok" }
  | { kind: "wrong" }
  | { kind: "rate_limited"; retryAfterSeconds: number | null }
  | { kind: "blocked"; where: string | null }
  | { kind: "broken"; detail: string }
  | { kind: "unreachable" }
  | { kind: "refused"; message: string }

async function post(path: string, body?: unknown): Promise<Response> {
  return fetch(apiUrl(path), {
    method: "POST",
    credentials: "same-origin",
    headers: body === undefined ? {} : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
}

export async function postLogin(password: string): Promise<LoginAnswer> {
  let resp: Response
  try {
    resp = await post("/api/v1/auth/login", { password })
  } catch {
    return { kind: "unreachable" }
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
    }
  }
  const where = blockedWhere(resp.status, body)
  if (where !== undefined) return { kind: "blocked", where }
  const detail = brokenDetail(resp.status, body)
  if (detail !== undefined) return { kind: "broken", detail }
  return {
    kind: "refused",
    message: str(body.json.message) ?? (body.text || `The server refused the sign-in (${resp.status}).`),
  }
}

export type LogoutAnswer =
  | { kind: "ok" }
  | { kind: "unreachable" }
  | { kind: "refused"; message: string }

export async function postLogout(): Promise<LogoutAnswer> {
  let resp: Response
  try {
    resp = await post("/api/v1/auth/logout")
  } catch {
    return { kind: "unreachable" }
  }
  if (resp.ok) return { kind: "ok" }
  const body = await readErrorBody(resp)
  // No session to end is the outcome a sign-out wanted.
  if (isAuthRequired(resp.status, body)) return { kind: "ok" }
  return {
    kind: "refused",
    message: body.text || `The server refused the sign-out (${resp.status}).`,
  }
}

export type PasswordAnswer =
  | { kind: "ok" }
  | { kind: "signed_out" }
  | { kind: "unreachable" }
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
  } catch {
    return { kind: "unreachable" }
  }
  if (resp.ok) return { kind: "ok" }
  const err = await readErrorBody(resp)
  if (isAuthRequired(resp.status, err)) return { kind: "signed_out" }
  const feedback = record(err.json.feedback)
  return {
    kind: "refused",
    message:
      str(err.json.message) ??
      str(feedback.warning) ??
      (err.text || `The server refused the password (${resp.status}).`),
    score: count(err.json.score),
  }
}

export async function postDismissNoAuthWarning(): Promise<void> {
  let resp: Response
  try {
    resp = await post("/api/v1/auth/dismiss-no-auth-warning")
  } catch {
    throw new Error("Could not reach the server.")
  }
  if (!resp.ok) {
    const body = await readErrorBody(resp)
    throw new Error(body.text || `request failed (${resp.status})`)
  }
}
