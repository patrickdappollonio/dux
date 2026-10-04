// The page's sign-in state, and the one place every unauthorized answer lands.
//
// The app boots only once this says `open`: a signed-in browser, or a server
// where no password applies to this connection. Signed out, blocked and broken
// each show a page of their own instead of the app, and while the page is in
// any of them nothing protected is sent (`authPaused`), sockets hold their
// retries, and no toast is raised.
//
// Every fetch helper reports through `apiFetch.ts` and every socket through its
// close code (4401 signed out or revoked, 4403 blocked), so a session that ends
// mid-use is noticed by whichever request or socket meets it first. A socket
// refused at the upgrade cannot tell the page why (the browser reports every
// failed upgrade as 1006), so a dropped app socket asks the status route
// (`probeAfterDrop`).
//
// The EPOCH moves each time the page leaves `open`. A request remembers the
// epoch it was sent in, and an answer that lands in a later one belongs to a
// session that is over, so it is dropped rather than applied.
//
// What signing out does NOT do: reload the page or touch `location.hash`. The
// store, the editor's drafts and the URL all live in page memory and survive a
// sign-out, so signing back in returns to exactly where the user was.

import { useSyncExternalStore } from "react"

import {
  type AuthStatus,
  fetchAuthStatus,
  normalizeAuthStatus,
  postLogin,
  postLogout,
  type LoginAnswer,
  type LogoutAnswer,
  type StatusAnswer,
} from "./authApi"

export { normalizeAuthStatus }
export type { AuthStatus }

/// The WebSocket close codes the server uses for auth: the session is gone
/// (signed out, revoked, expired), or the client address is blocked.
export const AUTH_REQUIRED_CLOSE = 4401
export const AUTH_BLOCKED_CLOSE = 4403

export function isAuthCloseCode(code: number): boolean {
  return code === AUTH_REQUIRED_CLOSE || code === AUTH_BLOCKED_CLOSE
}

/// Why the login page is up, which decides the sentence above the form.
export type SignOutReason =
  /** The first look found a password and no session. */
  | "required"
  /** A session that was in use ended underneath the page. */
  | "expired"
  /** The user signed out from the menu. */
  | "signed_out"
  /** The password changed, which signs every browser out. */
  | "password_changed"

export type AuthPhase =
  | { kind: "checking" }
  /** `status` is null when the server could not be asked. */
  | { kind: "open"; status: AuthStatus | null }
  | { kind: "signed_out"; status: AuthStatus | null; reason: SignOutReason }
  | { kind: "blocked"; where: string | null }
  | { kind: "broken"; detail: string }

/// What a status answer means for the page. Broken auth wins over everything:
/// a password the server cannot check is not a password the page can ask for.
export function phaseForStatus(status: AuthStatus): AuthPhase {
  if (status.auth_broken !== null) return { kind: "broken", detail: status.auth_broken }
  if (status.required_here && !status.signed_in) {
    return { kind: "signed_out", status, reason: "required" }
  }
  return { kind: "open", status }
}

let phase: AuthPhase = { kind: "checking" }
let epoch = 0
const listeners = new Set<() => void>()
const openListeners = new Set<() => void>()

function setPhase(next: AuthPhase): void {
  const wasOpen = phase.kind === "open"
  const isOpen = next.kind === "open"
  if (wasOpen && !isOpen) epoch++
  phase = next
  for (const l of [...listeners]) l()
  if (isOpen && !wasOpen) for (const l of [...openListeners]) l()
}

export function getAuthPhase(): AuthPhase {
  return phase
}

/// The session generation an answer is checked against; see the module doc.
export function authEpoch(): number {
  return epoch
}

/// Whether protected work must wait: the page is on its login, blocked or
/// broken screen. Not while the first answer is still coming, because nothing
/// protected is sent then anyway and a page that never asks must still work.
export function authPaused(): boolean {
  return phase.kind === "signed_out" || phase.kind === "blocked" || phase.kind === "broken"
}

export function subscribeAuth(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

/// Called on every move INTO `open`: the first boot and every sign-in after a
/// sign-out. The store boots or refetches; held sockets resume.
export function onAuthOpen(listener: () => void): () => void {
  openListeners.add(listener)
  return () => openListeners.delete(listener)
}

export function useAuthPhase(): AuthPhase {
  return useSyncExternalStore(subscribeAuth, getAuthPhase, getAuthPhase)
}

/// Whether this page holds a session it can end, which is what the app menu's
/// Sign out asks. Not merely "a password is set": a connection the password
/// does not apply to signed in with nothing.
export function hasSessionToEnd(p: AuthPhase): boolean {
  return p.kind === "open" && p.status !== null && p.status.password_set && p.status.signed_in
}

/// The latest status the page holds, whatever phase it is in.
export function currentAuthStatus(): AuthStatus | null {
  return phase.kind === "open" || phase.kind === "signed_out" ? phase.status : null
}

function applyAnswer(answer: StatusAnswer, signedOutReason: SignOutReason): void {
  switch (answer.kind) {
    case "status": {
      const next = phaseForStatus(answer.status)
      setPhase(next.kind === "signed_out" ? { ...next, reason: signedOutReason } : next)
      return
    }
    case "blocked":
      setPhase({ kind: "blocked", where: answer.where })
      return
    case "broken":
      setPhase({ kind: "broken", detail: answer.detail })
      return
    case "unknown":
      // No evidence either way. A page with nothing yet boots, and the
      // offline overlay or the next 401 says what is really wrong; a page
      // already somewhere stays there.
      if (phase.kind === "checking") setPhase({ kind: "open", status: null })
      return
  }
}

/// The first look, run once by the store at load.
export async function initAuthGate(): Promise<void> {
  applyAnswer(await fetchAuthStatus(), "required")
}

let probe: Promise<void> | null = null

/// Ask again and settle on the answer; one in flight however many ask.
export function probeAuth(reason: SignOutReason = "expired"): Promise<void> {
  if (probe) return probe
  probe = fetchAuthStatus()
    .then((answer) => applyAnswer(answer, reason))
    .finally(() => {
      probe = null
    })
  return probe
}

/// The app socket dropped: if the page thinks it is signed in, check, because
/// a refused upgrade looks exactly like a network failure from here.
export function probeAfterDrop(): Promise<void> {
  if (phase.kind !== "open") return Promise.resolve()
  return probeAuth("expired")
}

/// A protected request was answered 401: the session is gone.
export function reportUnauthorized(reason: SignOutReason = "expired"): void {
  if (phase.kind === "signed_out" || phase.kind === "blocked" || phase.kind === "broken") {
    return
  }
  setPhase({ kind: "signed_out", status: currentAuthStatus(), reason })
  // Refresh what the login page shows (the transport warning, the first
  // password rule); the session itself is already known to be over.
  void probeAuth(reason)
}

export function reportBlocked(where: string | null): void {
  setPhase({ kind: "blocked", where })
}

export function reportBroken(detail: string): void {
  setPhase({ kind: "broken", detail })
}

/// A socket closed with an auth code. 4403 carries no `where`, so the status
/// route is asked for it.
export function reportSocketAuthClose(code: number): void {
  if (code === AUTH_BLOCKED_CLOSE) {
    if (phase.kind !== "blocked") setPhase({ kind: "blocked", where: null })
    void probeAuth()
    return
  }
  if (code === AUTH_REQUIRED_CLOSE) reportUnauthorized()
}

export const COOKIE_NOT_KEPT =
  "The password was right, but this browser did not keep the sign-in. It may be blocking cookies for this address, or dux may be marking the cookie HTTPS-only while you reach it over plain HTTP. Allow cookies for this page, or check cookie_secure in the [server.auth] section of config.toml."

/// Sign in with the password. Success opens the page even when the follow-up
/// status read fails, because the login itself said yes.
export async function signIn(password: string): Promise<LoginAnswer> {
  const answer = await postLogin(password)
  switch (answer.kind) {
    case "ok": {
      const after = await fetchAuthStatus()
      if (after.kind === "unknown") {
        setPhase({ kind: "open", status: null })
        return answer
      }
      if (
        after.kind === "status" &&
        after.status.auth_broken === null &&
        after.status.required_here &&
        !after.status.signed_in
      ) {
        // The password was right and the session still did not stick: the
        // browser refused the cookie. Opening would only bounce straight back.
        setPhase({ kind: "signed_out", status: after.status, reason: "required" })
        return { kind: "refused", message: COOKIE_NOT_KEPT }
      }
      if (after.kind === "status" && after.status.auth_broken === null) {
        setPhase({ kind: "open", status: after.status })
        return answer
      }
      applyAnswer(after, "required")
      return answer
    }
    case "blocked":
      setPhase({ kind: "blocked", where: answer.where })
      return answer
    case "broken":
      setPhase({ kind: "broken", detail: answer.detail })
      return answer
    default:
      return answer
  }
}

/// End this browser's session. Only a server that answered moves the page to
/// the login screen: a sign-out that never arrived has not happened.
export async function signOut(): Promise<LogoutAnswer> {
  const answer = await postLogout()
  if (answer.kind === "ok") {
    setPhase({ kind: "signed_out", status: currentAuthStatus(), reason: "signed_out" })
    void probeAuth("signed_out")
  }
  return answer
}

/// The password changed and the server signed every browser out, this one
/// included. Asks where that leaves this page: a connection the password does
/// not apply to stays open.
export async function afterPasswordChange(): Promise<void> {
  const answer = await fetchAuthStatus()
  if (answer.kind === "status" && answer.status.required_here && !answer.status.signed_in) {
    setPhase({ kind: "signed_out", status: answer.status, reason: "password_changed" })
    return
  }
  applyAnswer(answer, "password_changed")
}

/// Replace the status an open page holds (a banner dismissed, a fresh read).
export function noteAuthStatus(status: AuthStatus): void {
  if (phase.kind === "open") setPhase({ kind: "open", status })
}
