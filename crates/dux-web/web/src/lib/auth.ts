// Pure auth helpers for the web frontend's login/auth-state machine.
//
// These are deliberately side-effect-free so they can be unit-tested without a
// DOM, a socket, or a live server. The store (store.ts) owns the fetch calls and
// the state transitions; this module owns the mappings those transitions depend
// on (parsing a `/api/me` body into a phase, turning a `Retry-After` header into
// a user-facing message, and choosing the right error string per HTTP status).

// The auth phases the SPA can be in. "checking" is the boot state while the
// first `/api/me` round-trip is in flight; the app shell does not render until it
// resolves to "disabled" or "authed" (so the WS connect always precedes the
// terminal's first subscribe — see store.ts). "unreachable" is the boot state
// when that round-trip fails at the network level (server down/restarting): the
// store auto-retries with capped backoff and the UI shows a reconnect affordance
// rather than masquerading the outage as a login prompt — see `bootAuth`.
export type AuthPhase =
  | "checking"
  | "disabled"
  | "anonymous"
  | "authed"
  | "unreachable"

export interface AuthState {
  phase: AuthPhase
  // The logged-in username when phase === "authed"; null otherwise.
  username: string | null
  // The last login error to surface on the login screen, or null. Cleared on a
  // new attempt and on success.
  error: string | null
  // Whether a login request is in flight (drives the submit button's disabled
  // state). Never persisted — purely transient UI feedback.
  pending: boolean
}

// The two 200 shapes `GET /api/me` can return (the 401 case is handled by the
// caller via the HTTP status, not the body).
//   - auth OFF        → { auth: "disabled" }
//   - auth ON, session→ { username: "..." }
export interface MeBody {
  auth?: string
  username?: string
}

// Map a resolved `/api/me` response into the boot auth phase. The caller passes
// the HTTP status and (for 200) the parsed body:
//   - 200 { auth: "disabled" } → "disabled"  (skip the login screen entirely)
//   - 200 { username }         → "authed"    (render the app, connect the WS)
//   - 401 (or anything else)   → "anonymous" (render the login screen)
// A 200 with neither field is treated as anonymous (defensive: an unexpected
// body should fail safe to the login screen rather than a half-authed app).
export function phaseFromMe(
  status: number,
  body: MeBody | null,
): { phase: AuthPhase; username: string | null } {
  if (status === 200 && body) {
    if (body.auth === "disabled") return { phase: "disabled", username: null }
    if (typeof body.username === "string" && body.username.length > 0) {
      return { phase: "authed", username: body.username }
    }
  }
  return { phase: "anonymous", username: null }
}

// Parse a `Retry-After` header value into whole seconds. The server sends a
// delta-seconds integer (per its rate limiter), so we parse that; an HTTP-date
// form is not produced by our server and is treated as the fallback. Returns a
// sane positive integer, defaulting to `fallback` when the header is missing or
// unparseable so the message always names a concrete wait.
export function parseRetryAfter(header: string | null, fallback = 60): number {
  if (header === null) return fallback
  const secs = Number.parseInt(header.trim(), 10)
  if (Number.isFinite(secs) && secs > 0) return secs
  return fallback
}

// Generic, user-enumeration-safe message for a rejected login. Mirrors the
// server's `LOGIN_FAILED_MESSAGE` so the UI never hints at whether the username
// exists.
export const LOGIN_INVALID_MESSAGE = "Invalid username or password."

// Choose the error message to show on the login screen for a failed login,
// given the HTTP status and (for 429) the parsed retry-after seconds:
//   - 401 → generic invalid-credentials message
//   - 429 → "Too many attempts — try again in N s"
//   - anything else (network/500) → a generic try-again message
export function loginErrorMessage(status: number, retryAfterSecs?: number): string {
  if (status === 401) return LOGIN_INVALID_MESSAGE
  if (status === 429) {
    const secs = retryAfterSecs ?? 60
    return `Too many attempts — try again in ${secs} s.`
  }
  return "Could not sign in. Please try again."
}

// The message shown when the login request itself fails to reach the server
// (fetch rejects: server down, connection dropped). Distinct from an HTTP error
// status, which `loginErrorMessage` handles.
export const LOGIN_NETWORK_MESSAGE = "Could not reach the server. Please try again."

// The capped-backoff schedule (milliseconds) for the boot `/api/me` auto-retry
// when the server is unreachable: 2s, 4s, 8s, then 10s forever. `attempt` is the
// zero-based count of retries already made; the returned delay is how long to
// wait before the NEXT attempt. Pure so the cadence is unit-testable without a
// timer. Kept deliberately short and simple (no jitter, no unbounded growth) —
// the goal is "recover quickly once the server is back" for a single-tenant dev
// tool, not to protect a fleet from a thundering herd.
const UNREACHABLE_BACKOFF_MS = [2000, 4000, 8000]
const UNREACHABLE_BACKOFF_MAX_MS = 10000

export function unreachableRetryDelay(attempt: number): number {
  return UNREACHABLE_BACKOFF_MS[attempt] ?? UNREACHABLE_BACKOFF_MAX_MS
}

// The status shown while the boot probe is failing and retrying.
export const UNREACHABLE_MESSAGE = "Can't reach dux — retrying…"
