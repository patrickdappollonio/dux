// The page's sign-in state, and the one place every unauthorized answer lands.
//
// The app boots only once this says `open`: a signed-in browser, or a server
// where no password applies to this connection. Signed out, blocked, broken,
// unreachable and stuck each show a page of their own over the app, and while
// the page is in any of them nothing protected is sent (`authPaused`), sockets
// hold their retries and their sends, and no toast is raised.
//
// Every fetch helper reports through `apiFetch.ts` and every socket through its
// close code (4401 signed out or revoked, 4403 blocked), so a session that ends
// mid-use is noticed by whichever request or socket meets it first. A socket
// refused at the upgrade cannot tell the page why (the browser reports every
// failed upgrade as 1006), so a dropped app socket asks the status route
// (`probeAfterDrop`).
//
// Two counters keep late answers in their place:
// - The EPOCH moves each time the page leaves `open`. A request remembers the
//   epoch it was sent in, so an ended session's refusal is not reported
//   against the session that replaced it (`apiFetch.ts`).
// - The GENERATION moves on every phase change that is not a probe's own
//   answer: a sign-in, a sign-out, a refusal, a socket close. A status probe
//   remembers the generation it was asked in, and its answer is dropped if the
//   page moved since, or if an answer to a probe asked LATER has already
//   landed (probes are numbered, so an old answer never undoes a newer one).
//
// THE CIRCUIT BREAKER. A protected route that refuses the session while the
// status route says the session is fine would otherwise loop: sign out, read
// the status, reopen, refuse again. Each such disagreement reopens only after
// an exponential delay, and the third without a quiet minute between them
// stops on a page that says so. Other requests succeeding meanwhile prove
// nothing (the reopen's own refetches succeed every time), so only quiet
// forgets the count.
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
  type LoginAnswer,
  type StatusAnswer,
} from "./authApi"
import {
  postLogout,
  postPassword,
  type LogoutAnswer,
  type PasswordAnswer,
} from "./authActions"

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
  /** `status` is null when the server answered without the document. */
  | { kind: "open"; status: AuthStatus | null }
  | { kind: "signed_out"; status: AuthStatus | null; reason: SignOutReason }
  | { kind: "blocked"; where: string | null }
  | { kind: "broken"; detail: string }
  /** The first look got no answer at all. */
  | { kind: "unreachable"; timedOut: boolean }
  /** The circuit breaker tripped: the server keeps refusing a session its
   * own status route calls valid. */
  | { kind: "stuck" }

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
let generation = 0
let probeSeq = 0
let appliedProbeSeq = 0
const listeners = new Set<() => void>()
const openListeners = new Set<() => void>()

function setPhase(next: AuthPhase, fromProbe = false): void {
  const wasOpen = phase.kind === "open"
  const isOpen = next.kind === "open"
  if (wasOpen && !isOpen) epoch++
  if (!fromProbe) generation++
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

/// Whether protected work must wait: the page is on one of the gate's own
/// pages. Not while the first answer is still coming, because nothing
/// protected is sent then anyway and a page that never asks must still work.
export function authPaused(): boolean {
  return phase.kind !== "open" && phase.kind !== "checking"
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

// ---- The circuit breaker ----------------------------------------------------

/// Disagreements, each within the quiet period of the last, before the page
/// stops on the stuck page.
export const MAX_AUTH_DISAGREEMENTS = 3
/// The first reopen delay; each disagreement doubles it.
export const AUTH_DISAGREEMENT_BASE_MS = 1000
/// How long without a disagreement before the count is forgotten.
export const AUTH_DISAGREEMENT_QUIET_MS = 60_000

let disagreements = 0
let lastDisagreementAt = 0

// ---- Applying answers -------------------------------------------------------

// Who is applying: the first look, a direct read of its own (a sign-in, a
// password change), a probe, or a probe that follows a refusal.
type Origin = "init" | "direct" | "probe" | "after_refusal"

function applyAnswer(answer: StatusAnswer, reason: SignOutReason, origin: Origin): void {
  // A probe's answer moves the page without moving the generation, so a newer
  // probe still in flight is not invalidated by an older one landing first.
  const fromProbe = origin === "probe" || origin === "after_refusal"
  switch (answer.kind) {
    case "status": {
      const next = phaseForStatus(answer.status)
      if (next.kind === "signed_out") {
        setPhase({ ...next, reason }, fromProbe)
        return
      }
      if (next.kind === "open" && origin === "after_refusal") {
        reopenAfterDisagreement(next)
        return
      }
      setPhase(next, fromProbe)
      return
    }
    case "blocked":
      setPhase({ kind: "blocked", where: answer.where }, fromProbe)
      return
    case "broken":
      setPhase({ kind: "broken", detail: answer.detail }, fromProbe)
      return
    case "unknown":
      // The server answered without the document (an older dux): no password
      // to ask for. A page already somewhere stays there.
      if (phase.kind === "checking") setPhase({ kind: "open", status: null }, fromProbe)
      return
    case "unreachable":
      // A page already somewhere stays there, and the offline overlay speaks.
      if (phase.kind !== "checking") return
      // On the first look, a request that failed at once (the server is down,
      // the network is off) boots the app, whose offline overlay already says
      // that and keeps retrying; one that hung until the deadline gets its own
      // page with a Retry, because nothing else would ever say anything.
      setPhase(
        answer.timedOut
          ? { kind: "unreachable", timedOut: true }
          : { kind: "open", status: null },
        fromProbe,
      )
      return
  }
}

// A protected route refused the session and the status route says it is
// fine. Reopen, but only after a delay that doubles each time, and stop on the
// stuck page once it has happened often enough.
function reopenAfterDisagreement(next: AuthPhase): void {
  const now = Date.now()
  if (now - lastDisagreementAt > AUTH_DISAGREEMENT_QUIET_MS) disagreements = 0
  lastDisagreementAt = now
  disagreements++
  if (disagreements >= MAX_AUTH_DISAGREEMENTS) {
    setPhase({ kind: "stuck" })
    return
  }
  const at = generation
  setTimeout(
    () => {
      if (generation === at) setPhase(next)
    },
    AUTH_DISAGREEMENT_BASE_MS * 2 ** (disagreements - 1),
  )
}

/// The first look, run once by the store at load.
export async function initAuthGate(): Promise<void> {
  const at = generation
  const answer = await fetchAuthStatus()
  if (generation !== at) return
  applyAnswer(answer, "required", "init")
}

/// Look again from a gate page that has a Retry: the unreachable page, the
/// stuck page, the blocked and broken pages.
export async function retryAuthGate(): Promise<void> {
  disagreements = 0
  lastDisagreementAt = 0
  if (phase.kind === "unreachable" || phase.kind === "stuck") setPhase({ kind: "checking" })
  if (phase.kind === "checking") {
    await initAuthGate()
    return
  }
  await probeAuth("required", { fresh: true })
}

let probe: { generation: number; promise: Promise<void> } | null = null

/// Ask again and settle on the answer. Callers asking in the same generation
/// share one request; `fresh` always asks anew (a Try again must not wait on a
/// request that may be hanging). An answer that lands after the page moved is
/// dropped, whoever asked for it.
export function probeAuth(
  reason: SignOutReason = "expired",
  opts: { fresh?: boolean; origin?: Origin } = {},
): Promise<void> {
  if (!opts.fresh && probe !== null && probe.generation === generation) return probe.promise
  const at = generation
  const seq = ++probeSeq
  const origin = opts.origin ?? "probe"
  const promise = fetchAuthStatus().then((answer) => {
    if (generation !== at || seq < appliedProbeSeq) return
    appliedProbeSeq = seq
    applyAnswer(answer, reason, origin)
  })
  const entry = { generation: at, promise }
  probe = entry
  void promise.finally(() => {
    if (probe === entry) probe = null
  })
  return promise
}

// Set while an explicit sign-out or password change is out: everything it
// causes (the server closing this page's sockets, the drop probes that follow)
// reads as the act it is, not as a session that ended by itself, and the act's
// own outcome is what the page settles on.
let signingOut = false
let changingPassword = false

function actInFlight(): boolean {
  return signingOut || changingPassword
}

/// The app socket dropped: if the page thinks it is signed in, check, because
/// a refused upgrade looks exactly like a network failure from here.
export function probeAfterDrop(): Promise<void> {
  if (phase.kind !== "open" || actInFlight()) return Promise.resolve()
  return probeAuth("expired")
}

/// The status of an open page, read again: Preferences opening, the banners
/// showing, a config change announced by the server.
export function refreshAuthStatus(): Promise<void> {
  if (phase.kind !== "open") return Promise.resolve()
  return probeAuth("expired", { fresh: true })
}

/// A protected request was answered 401: the session is gone.
export function reportUnauthorized(): void {
  if (phase.kind !== "open" && phase.kind !== "checking") return
  const reason: SignOutReason = signingOut
    ? "signed_out"
    : changingPassword
      ? "password_changed"
      : "expired"
  setPhase({ kind: "signed_out", status: currentAuthStatus(), reason })
  // Refresh what the login page shows (the transport warning, the first
  // password rule). During a sign-out the answer is only a refresh; otherwise
  // a "signed in" answer is a disagreement for the breaker.
  void probeAuth(reason, { origin: actInFlight() ? "probe" : "after_refusal" })
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
      disagreements = 0
      const after = await fetchAuthStatus()
      if (after.kind === "unknown" || after.kind === "unreachable") {
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
      applyAnswer(after, "required", "direct")
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

/// What a sign-out came to: the contract's answers, `reopened` when this
/// connection needs no password (so dux opened again, and the caller says so),
/// and `gate` when the server answered with something the gate now shows (a
/// blocked address, a broken config).
export type SignOutAnswer =
  | { kind: "ok"; reopened?: true }
  | { kind: "gate" }
  | Exclude<LogoutAnswer, { kind: "ok" } | { kind: "gate" }>

/// End this browser's session. The reason is settled before the request goes
/// out, so the socket closes and drop probes the sign-out itself causes cannot
/// relabel it "your session ended". Only a server that answered moves the page
/// to the login screen: a sign-out that never arrived has not happened.
export async function signOut(): Promise<SignOutAnswer> {
  signingOut = true
  try {
    const answer = await postLogout()
    if (answer.kind !== "ok") return answer
    const after = await fetchAuthStatus()
    if (after.kind === "blocked" || after.kind === "broken") {
      applyAnswer(after, "signed_out", "direct")
      return { kind: "gate" }
    }
    if (
      after.kind === "status" &&
      after.status.auth_broken === null &&
      !after.status.required_here
    ) {
      // Nothing to sign in to from here: say so rather than flicker.
      setPhase({ kind: "open", status: after.status })
      return { kind: "ok", reopened: true }
    }
    setPhase({
      kind: "signed_out",
      status: after.kind === "status" ? after.status : currentAuthStatus(),
      reason: "signed_out",
    })
    return { kind: "ok" }
  } finally {
    signingOut = false
  }
}

/// Change (or set the first) password. Success signs every browser out, this
/// one included, and the server's socket closes may well land before its
/// answer; the change's outcome is what the page settles on, so the login page
/// says the password changed. A connection the password does not apply to
/// stays open.
export async function changePassword(write: {
  current?: string
  next: string
}): Promise<PasswordAnswer> {
  changingPassword = true
  try {
    const answer = await postPassword(write)
    if (answer.kind !== "ok") return answer
    const after = await fetchAuthStatus()
    if (after.kind === "status" && after.status.required_here && !after.status.signed_in) {
      setPhase({ kind: "signed_out", status: after.status, reason: "password_changed" })
    } else if (after.kind === "status" || after.kind === "blocked" || after.kind === "broken") {
      applyAnswer(after, "password_changed", "direct")
    }
    return answer
  } finally {
    changingPassword = false
  }
}
