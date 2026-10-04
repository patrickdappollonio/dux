// The one door every protected HTTP request goes through. It resolves the URL
// (`apiBase.ts`), refuses to send while the page is signed out, blocked or
// broken, reports the contract's auth answers to the gate, and drops an answer
// that lands after the session it was asked in has ended. Everything else (the
// status, the body, a network failure) reaches the caller exactly as `fetch`
// would have handed it over.
//
// The auth routes themselves do not come through here (`authApi.ts`): they must
// work while signed out, and their 401 means a wrong password.

import { apiUrl } from "./apiBase"
import { blockedWhere, brokenDetail, isAuthRequired, readErrorBody } from "./authApi"
import {
  authEpoch,
  authPaused,
  reportBlocked,
  reportBroken,
  reportUnauthorized,
} from "./authGate"

/// A request that did not get to finish because the page is (or was, while it
/// waited) not signed in. Callers need not special-case it: the page is already
/// showing the login, blocked or broken screen, and notifications are held
/// while it does. `isAuthInterruption` is there for a caller that wants to stay
/// quiet about it anyway.
export class AuthInterruptedError extends Error {
  readonly status = 401
  constructor() {
    super("Signed out of dux before the server answered. Sign in again to continue.")
    this.name = "AuthInterruptedError"
  }
}

export function isAuthInterruption(e: unknown): e is AuthInterruptedError {
  return e instanceof AuthInterruptedError
}

export async function apiFetch(path: string, init?: RequestInit): Promise<Response> {
  if (authPaused()) throw new AuthInterruptedError()
  const sentIn = authEpoch()
  let resp: Response
  try {
    resp = init === undefined ? await fetch(apiUrl(path)) : await fetch(apiUrl(path), init)
  } catch (e) {
    if (authEpoch() !== sentIn) throw new AuthInterruptedError()
    throw e
  }
  if (authEpoch() !== sentIn) throw new AuthInterruptedError()
  if (resp.status === 401 || resp.status === 403 || resp.status === 503) {
    const body = await readErrorBody(resp)
    if (isAuthRequired(resp.status, body)) {
      reportUnauthorized()
      throw new AuthInterruptedError()
    }
    const where = blockedWhere(resp.status, body)
    if (where !== undefined) {
      reportBlocked(where)
      throw new AuthInterruptedError()
    }
    const detail = brokenDetail(resp.status, body)
    if (detail !== undefined) {
      reportBroken(detail)
      throw new AuthInterruptedError()
    }
  }
  return resp
}
