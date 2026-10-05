import { useEffect, useId, useRef, useState, type ReactNode } from "react"
import { ShieldAlert } from "lucide-react"

import { Button } from "@/components/ui/button"
import { InlineCode } from "@/components/ui/inline-code"
import { renderInlineCode } from "@/lib/inlineMarkdown"
import { Input } from "@/components/ui/input"
import type { AuthStatus, LoginAnswer } from "@/lib/authApi"
import { rateLimitLead } from "@/lib/authErrors"
import { retryAuthGate, signIn, type SignOutReason } from "@/lib/authGate"
import { DEFAULT_FAVICON_HREF } from "@/lib/favicon"

// The pages the sign-in gate shows instead of the app: the login form, the
// blocked page and the broken-config page. In dux's own look (theme tokens and
// the shared primitives), centered on the app background, one column wide.
//
// None of them touches `location.hash`: the URL keeps naming where the user
// was, and the app lands there once it opens.

function Shell({
  title,
  logo = true,
  children,
}: {
  title: string
  /// Whether to show dux's logo, which is a request to the server.
  logo?: boolean
  children: ReactNode
}) {
  return (
    <main className="flex min-h-svh items-center justify-center bg-background px-4 py-10 text-foreground">
      <div className="flex w-full max-w-sm flex-col gap-5">
        <div className="flex flex-col items-center gap-3 text-center">
          {logo ? <img src={DEFAULT_FAVICON_HREF} alt="" className="size-12" /> : null}
          <h1 className="text-lg font-semibold">{title}</h1>
        </div>
        {children}
      </div>
    </main>
  )
}

const REASON: Record<SignOutReason, string> = {
  required: "This dux asks for a password before it lets anyone in.",
  expired: "Your session ended. Sign in again to pick up where you left off.",
  signed_out: "You signed out. Sign in again to pick up where you left off.",
  password_changed:
    "The password changed, which signs every browser out. Sign in with the new one.",
  // The one confirmation of a first password that now applies here: notices
  // are held while the gate is up, so the page itself says it.
  password_set: "Password set. Sign in with it to continue.",
}

// Shown only when the server says this connection is NOT encrypted: not on
// loopback, not on the tailnet, not over HTTPS. An unknown answer shows
// nothing rather than a warning that may be false.
function PlainHttpWarning() {
  const id = useId()
  return (
    <div
      role="note"
      data-testid="login-insecure-warning"
      aria-labelledby={id}
      className="flex gap-3 rounded-lg border border-destructive/50 bg-destructive/10 p-3 text-sm"
    >
      <ShieldAlert className="mt-0.5 size-4 shrink-0 text-destructive" aria-hidden />
      <div className="flex flex-col gap-1">
        <p id={id} className="font-medium text-destructive">
          This connection is not encrypted
        </p>
        <p className="text-muted-foreground">
          You reached dux over plain HTTP. Anyone on the network between you and
          it can read the password as you type it, take the session cookie after
          you sign in, and change this page before it reaches you. Reach dux over
          HTTPS or your tailnet instead if you can.
        </p>
      </div>
    </div>
  )
}

type Failure = Exclude<LoginAnswer, { kind: "ok" }> | { kind: "empty" }

// The sentence the alert region announces. A rate limit's countdown is NOT in
// it: the alert is read once, and the seconds tick in a quiet element beside it.
function failureText(failure: Failure): string {
  switch (failure.kind) {
    case "empty":
      return "Enter your password."
    case "wrong":
      return "That password did not work. Try again."
    case "rate_limited":
      return rateLimitLead(failure.from)
    case "unreachable":
      return failure.timedOut
        ? "dux did not answer in time. Check that it is still running, then try again."
        : "Could not reach dux. Check that it is still running, then try again."
    case "refused":
      return failure.message
    case "blocked":
    case "broken":
      // The gate has already replaced this page with the right one.
      return ""
  }
}

function countdownText(seconds: number | null): string {
  if (seconds === null) return "Wait a little, then try again."
  if (seconds <= 0) return "You can try again now."
  return `Try again in ${seconds} ${seconds === 1 ? "second" : "seconds"}.`
}

export function LoginPage({
  status,
  reason,
}: {
  status: AuthStatus | null
  reason: SignOutReason
}) {
  const fieldId = useId()
  const [password, setPassword] = useState("")
  const [busy, setBusy] = useState(false)
  const [failure, setFailure] = useState<Failure | null>(null)
  // When a rate limit lifts, as `Date.now()`, and the seconds left until then.
  const [retryAt, setRetryAt] = useState<number | null>(null)
  const [secondsLeft, setSecondsLeft] = useState<number | null>(null)
  const fieldRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (retryAt === null) return
    const tick = () => {
      const left = Math.max(0, Math.ceil((retryAt - Date.now()) / 1000))
      setSecondsLeft(left)
      if (left === 0) setRetryAt(null)
    }
    const timer = setInterval(tick, 1000)
    return () => clearInterval(timer)
  }, [retryAt])

  const waiting = retryAt !== null && (secondsLeft ?? 1) > 0

  // Back to the field after a failure, once it is enabled again: a disabled
  // input refuses focus, so focusing it in the same tick as the answer did
  // nothing.
  useEffect(() => {
    if (!busy && failure !== null) fieldRef.current?.focus()
  }, [busy, failure])

  const submit = async (e: React.FormEvent) => {
    e.preventDefault()
    if (busy || waiting) return
    if (password === "") {
      setFailure({ kind: "empty" })
      return
    }
    setBusy(true)
    setFailure(null)
    const answer = await signIn(password)
    setBusy(false)
    if (answer.kind === "ok") {
      setPassword("")
      return
    }
    if (answer.kind === "rate_limited" && answer.retryAfterSeconds !== null) {
      setRetryAt(Date.now() + answer.retryAfterSeconds * 1000)
      setSecondsLeft(answer.retryAfterSeconds)
    } else {
      setRetryAt(null)
      setSecondsLeft(null)
    }
    if (answer.kind === "wrong") setPassword("")
    setFailure(answer)
  }

  const message = failure === null ? "" : failureText(failure)

  return (
    <Shell title="Sign in to dux">
      <p className="text-center text-sm text-muted-foreground">{REASON[reason]}</p>
      {status?.required_reason ? (
        // Why this device signs in at all: it reached dux over loopback, but
        // dux could not rule out a relay from elsewhere.
        <p
          data-testid="login-required-reason"
          className="text-center text-sm text-muted-foreground"
        >
          {renderInlineCode(status.required_reason)}
        </p>
      ) : null}
      {status?.transport_encrypted === false ? <PlainHttpWarning /> : null}
      <form
        data-testid="login-form"
        className="flex flex-col gap-3"
        onSubmit={(e) => void submit(e)}
        noValidate
      >
        {/* dux has one owner and no user names, but password managers file a
            password under a user name; this gives them one. Off-screen rather
            than `hidden`, which some managers skip, and out of the tab order
            and the accessibility tree, because nobody types in it. */}
        <input
          type="text"
          name="username"
          autoComplete="username"
          value="dux"
          readOnly
          tabIndex={-1}
          aria-hidden="true"
          className="sr-only"
        />
        <label htmlFor={fieldId} className="text-sm font-medium">
          Password
        </label>
        <Input
          ref={fieldRef}
          id={fieldId}
          name="password"
          type="password"
          autoComplete="current-password"
          autoFocus
          value={password}
          disabled={busy}
          aria-invalid={failure?.kind === "wrong" || failure?.kind === "empty" ? true : undefined}
          aria-describedby={message ? `${fieldId}-error` : undefined}
          onChange={(e) => setPassword(e.target.value)}
          className="h-10"
        />
        {message ? (
          <p id={`${fieldId}-error`} role="alert" className="text-sm text-destructive">
            {message}
          </p>
        ) : null}
        {failure?.kind === "rate_limited" ? (
          <p className="text-sm text-muted-foreground">
            {countdownText(retryAt === null && secondsLeft === null ? null : secondsLeft)}
          </p>
        ) : null}
        {/* Filled: committing the password is the page's one primary act. */}
        <Button type="submit" className="h-10" disabled={busy || waiting}>
          Sign in
        </Button>
      </form>
    </Shell>
  )
}

// Every gate page's way out. It always asks afresh, so a request still hanging
// from before cannot hold it, and it comes back enabled however that ends.
function RetryButton({ label }: { label: string }) {
  const [busy, setBusy] = useState(false)
  return (
    <Button
      variant="outline"
      className="h-10"
      disabled={busy}
      onClick={() => {
        setBusy(true)
        void retryAuthGate().finally(() => setBusy(false))
      }}
    >
      {label}
    </Button>
  )
}

export function BlockedPage({ where }: { where: string | null }) {
  return (
    // No logo: a blocked address is refused every request, the logo's
    // included, so the blocked state asks the server for nothing. It matches
    // the page the server itself sends a fresh load from a blocked address,
    // which has none either.
    <Shell title="This address is blocked" logo={false}>
      <p className="text-sm text-muted-foreground">
        dux refuses every request from the address you are connecting from.{" "}
        {/* The server's `where` is a whole location phrase that already names
          * the setting and its section, so it is the sentence's object as it
          * came, never slotted into a phrase of the page's own. */}
        {where ? (
          <>The block is an entry in {where}.</>
        ) : (
          <>
            The block is an entry in <InlineCode>blocked_addresses</InlineCode>, in
            the <InlineCode>[server.auth]</InlineCode> section of{" "}
            <InlineCode>config.toml</InlineCode>.
          </>
        )}{" "}
        Whoever runs dux can remove the address there and reload the config; dux
        also adds an address on its own after too many failed sign-ins.
      </p>
      <RetryButton label="Try again" />
    </Shell>
  )
}

// The detail is shown only when the server sent one, which it does only to
// this machine and the tailnet; everyone else gets the same sentence without
// it, so the page never depends on it.
export function BrokenPage({ detail }: { detail: string }) {
  return (
    <Shell title="Sign-in is misconfigured">
      <p className="text-sm text-muted-foreground">
        dux cannot check passwords because the{" "}
        <InlineCode>[server.auth]</InlineCode> section of its config is invalid,
        so it refuses every protected request rather than letting anyone in.
        Fix the section in <InlineCode>config.toml</InlineCode> on the machine dux
        runs on, reload the config, then try again.
      </p>
      {detail ? (
        <pre className="overflow-x-auto rounded-lg border border-border bg-muted/40 p-3 font-mono text-xs whitespace-pre-wrap wrap-anywhere">
          {detail}
        </pre>
      ) : null}
      <RetryButton label="Try again" />
    </Shell>
  )
}

export function UnreachablePage({ timedOut }: { timedOut: boolean }) {
  return (
    <Shell title="Can't reach dux">
      <p className="text-sm text-muted-foreground">
        {timedOut
          ? "dux did not answer in time, so this page cannot tell whether it needs a password yet. "
          : "This page could not reach dux to ask whether it needs a password. "}
        Check that dux is still running and that this device can reach it, then
        retry.
      </p>
      <RetryButton label="Retry" />
    </Shell>
  )
}

export function StuckPage() {
  return (
    <Shell title="dux keeps refusing this browser">
      <p className="text-sm text-muted-foreground">
        dux says this browser is signed in, then refuses its requests as if it
        were not, several times in a row. Rather than keep flipping between the
        app and the sign-in page, this page has stopped. Something between this
        browser and dux may be dropping the session cookie, or dux may have just
        restarted. Try again, and if this keeps happening, check{" "}
        <InlineCode>dux.log</InlineCode>.
      </p>
      <RetryButton label="Try again" />
    </Shell>
  )
}

export function CheckingPage() {
  return (
    <Shell title="Connecting to dux…">
      <p className="text-center text-sm text-muted-foreground">
        Asking dux whether this browser needs to sign in.
      </p>
    </Shell>
  )
}
