// The one door every protected HTTP request goes through. It resolves the URL
// (`apiBase.ts`), refuses to send while the page is signed out, blocked or
// broken, and reports the contract's auth refusals to the gate. Everything else
// (the status, the body, a network failure) reaches the caller exactly as
// `fetch` would have handed it over.
//
// A SUCCESSFUL answer is always delivered, even when the session it was sent in
// has ended since: the server did the work, and calling a completed mutation
// interrupted would be a lie. Reads that could overwrite newer state are
// already ordered by their own sequence guards in the store. What an ended
// session's answer may NOT do is speak for the new one: its auth refusal is
// not reported to the gate, because a 401 for the old cookie says nothing about
// the session the user has just started.
//
// The public auth routes do not come through here (`authApi.ts`): they must
// work while signed out, and their 401 means a wrong password.

import { apiUrl } from "./apiBase"
import { brokenDetail, isAuthRequired, isBlocked, readErrorBody } from "./authErrors"
import {
  authEpoch,
  authPaused,
  reportBlocked,
  reportBroken,
  reportUnauthorized,
} from "./authGate"

/// A request that did not get to finish because the page is not signed in.
/// `not_sent`: refused before it left, so nothing happened on the server.
/// `refused`: the server answered that the session is gone, so nothing
/// happened there either. Callers need not special-case it: the page is
/// already showing the login, blocked or broken screen, and notifications are
/// held while it does.
export class AuthInterruptedError extends Error {
  readonly status = 401
  readonly reason: "not_sent" | "refused"
  /// What the server said, for a refusal: the session is gone, the address is
  /// blocked, or the auth config is broken. Null for a request never sent.
  readonly refusal: "signed_out" | "blocked" | "broken" | null
  constructor(
    reason: "not_sent" | "refused",
    refusal: "signed_out" | "blocked" | "broken" | null = null,
  ) {
    super(
      reason === "not_sent"
        ? "This was not sent: this browser is signed out of dux. Sign in again, then retry."
        : "dux refused this because this browser's session ended, so nothing was done. Sign in again, then retry.",
    )
    this.name = "AuthInterruptedError"
    this.reason = reason
    this.refusal = refusal
  }
}

export function isAuthInterruption(e: unknown): e is AuthInterruptedError {
  return e instanceof AuthInterruptedError
}

/// For a helper that turns a thrown fetch into its own "could not reach the
/// server" error: an auth interruption is not that, and must reach the caller as
/// itself.
export function rethrowAuthInterruption(e: unknown): void {
  if (isAuthInterruption(e)) throw e
}

export async function apiFetch(path: string, init?: RequestInit): Promise<Response> {
  if (authPaused()) throw new AuthInterruptedError("not_sent")
  const sentIn = authEpoch()
  const resp =
    init === undefined ? await fetch(apiUrl(path)) : await fetch(apiUrl(path), init)
  const current = authEpoch() === sentIn
  if (resp.ok) return resp
  if (resp.status !== 401 && resp.status !== 403 && resp.status !== 503) return resp
  const body = await readErrorBody(resp)
  if (isAuthRequired(resp.status, body)) {
    if (current) reportUnauthorized()
    throw new AuthInterruptedError("refused", "signed_out")
  }
  if (isBlocked(resp.status, body)) {
    // About the address, not the session, so it holds whichever session asked.
    reportBlocked()
    throw new AuthInterruptedError("refused", "blocked")
  }
  const detail = brokenDetail(resp.status, body)
  if (detail !== undefined) {
    reportBroken(detail)
    throw new AuthInterruptedError("refused", "broken")
  }
  return resp
}
