// How an auth refusal reads, for the routes and the door that meet them. Pure:
// no fetch, no state, so `authApi.ts`, `authActions.ts` and `apiFetch.ts` can
// all share it without importing each other.
//
// Every contract error code becomes a sentence. A body that is JSON is never
// shown as it came: a code nobody mapped reads as a generic refusal naming the
// action and the status, so a new server code degrades to words, not braces.

export function record(raw: unknown): Record<string, unknown> {
  return typeof raw === "object" && raw !== null ? (raw as Record<string, unknown>) : {}
}

export function str(v: unknown): string | null {
  return typeof v === "string" && v !== "" ? v : null
}

export function count(v: unknown): number | null {
  return typeof v === "number" && Number.isFinite(v) ? v : null
}

/// A refusal body as the contract shapes it, plus the raw text for anything
/// that is not JSON (an older route, a proxy's page).
export interface ErrorBody {
  json: Record<string, unknown>
  text: string
}

function parseRecord(text: string): Record<string, unknown> {
  try {
    return record(JSON.parse(text))
  } catch {
    return {}
  }
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

/// Whether a refusal is the contract's "this browser has no valid session".
/// A 401 with no readable reason counts: no other dux route answers 401
/// without one.
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

/// What a password change that was stored but is not in force comes to: the
/// file holds the new password, but problems in it stop dux from using it, so
/// the old one (or none) still applies. Leads with that fact, then the server's
/// sentence, which names the problems.
export function storedNotInForceSentence(server: string | null): string {
  const lead = "The password was stored but is not in force."
  return server === null
    ? `${lead} Problems in config.toml stop dux from using it; fix them, and the old password applies until then.`
    : `${lead} ${server}`
}

/// Why the first password cannot be set from this page, with the server's
/// line about this device when it gave one. Setting names in backticks, for
/// the chip renderer.
export function firstPasswordUnavailable(requiredReason: string | null): string {
  const base =
    "No password is set. The first one can only be set from this machine, from your tailnet, or by running `dux config set server.auth.password` where dux runs."
  return requiredReason === null ? base : `${base} ${requiredReason}`
}

export function rateLimitSentence(seconds: number | null): string {
  if (seconds === null) return "Too many attempts from this address. Wait a little, then try again."
  if (seconds <= 0) return "Too many attempts from this address. You can try again now."
  return `Too many attempts from this address. Try again in ${seconds} ${seconds === 1 ? "second" : "seconds"}.`
}

function looksLikeJson(text: string): boolean {
  return text.startsWith("{") || text.startsWith("[")
}

/// The sentence for a refusal of `action` ("sign out", "change the password").
export function refusalSentence(
  status: number,
  body: ErrorBody,
  action: string,
  retryAfterHeader: string | null = null,
): string {
  const code = str(body.json.error)
  switch (code) {
    case "wrong_current_password":
      return "The current password is not right, so nothing was changed."
    case "password_not_in_force":
      return str(body.json.message) ?? storedNotInForceSentence(null)
    case "weak_password": {
      const feedback = record(body.json.feedback)
      const suggestions = Array.isArray(feedback.suggestions) ? feedback.suggestions : []
      const extra = [str(feedback.warning), str(suggestions[0])].filter(
        (x): x is string => x !== null,
      )
      return ["That password is too easy to guess, so dux refused it.", ...extra].join(" ")
    }
    case "password_too_short": {
      const n =
        count(body.json.minimum_length) ??
        count(body.json.minimum_password_length) ??
        count(body.json.minimum)
      return n === null
        ? "That password is shorter than the minimum dux asks for."
        : `That password is too short: dux asks for at least ${n} characters.`
    }
    case "blocked":
      return "dux refuses requests from this address. Whoever runs dux can lift the block by removing it from blocked_addresses in config.toml."
    case "auth_config_invalid":
      return "Sign-in is misconfigured on the server, so dux refuses this until the [server.auth] section of its config is fixed."
    case "auth_required":
      return "Your session ended. Sign in again, then try once more."
    case "rate_limited":
      return rateLimitSentence(retryAfterSeconds(retryAfterHeader, body.json, Date.now()))
  }
  if (status === 429) {
    return rateLimitSentence(retryAfterSeconds(retryAfterHeader, body.json, Date.now()))
  }
  const message = str(body.json.message)
  if (message !== null) return message
  if (body.text !== "" && !looksLikeJson(body.text)) return body.text
  return `dux refused to ${action} (HTTP ${status}).`
}
