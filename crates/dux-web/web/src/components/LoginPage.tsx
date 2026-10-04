import { useEffect, useId, useRef, useState, type ReactNode } from "react"
import { ShieldAlert } from "lucide-react"

import { Button } from "@/components/ui/button"
import { InlineCode } from "@/components/ui/inline-code"
import { Input } from "@/components/ui/input"
import type { AuthStatus, LoginAnswer } from "@/lib/authApi"
import { probeAuth, signIn, type SignOutReason } from "@/lib/authGate"
import { DEFAULT_FAVICON_HREF } from "@/lib/favicon"

// The pages the sign-in gate shows instead of the app: the login form, the
// blocked page and the broken-config page. In dux's own look (theme tokens and
// the shared primitives), centered on the app background, one column wide.
//
// None of them touches `location.hash`: the URL keeps naming where the user
// was, and the app lands there once it opens.

function Shell({ title, children }: { title: string; children: ReactNode }) {
  return (
    <main className="flex min-h-svh items-center justify-center bg-background px-4 py-10 text-foreground">
      <div className="flex w-full max-w-sm flex-col gap-5">
        <div className="flex flex-col items-center gap-3 text-center">
          <img src={DEFAULT_FAVICON_HREF} alt="" className="size-12" />
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

function failureText(answer: Exclude<LoginAnswer, { kind: "ok" }>, seconds: number | null): string {
  switch (answer.kind) {
    case "wrong":
      return "That password did not work. Try again."
    case "rate_limited":
      return seconds !== null && seconds > 0
        ? `Too many attempts from this address. Try again in ${seconds} ${seconds === 1 ? "second" : "seconds"}.`
        : seconds === null
          ? "Too many attempts from this address. Wait a little, then try again."
          : "You can try again now."
    case "unreachable":
      return "Could not reach dux. Check that it is still running, then try again."
    case "refused":
      return answer.message
    case "blocked":
    case "broken":
      // The gate has already replaced this page with the right one.
      return ""
  }
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
  const [failure, setFailure] = useState<Exclude<LoginAnswer, { kind: "ok" }> | null>(null)
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

  const submit = async (e: React.FormEvent) => {
    e.preventDefault()
    if (busy || waiting || password === "") return
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
    fieldRef.current?.focus()
  }

  const message =
    failure === null
      ? ""
      : failureText(failure, failure.kind === "rate_limited" ? secondsLeft : null)

  return (
    <Shell title="Sign in to dux">
      <p className="text-center text-sm text-muted-foreground">{REASON[reason]}</p>
      {status?.transport_encrypted === false ? <PlainHttpWarning /> : null}
      <form
        data-testid="login-form"
        className="flex flex-col gap-3"
        onSubmit={(e) => void submit(e)}
        noValidate
      >
        <label htmlFor={fieldId} className="text-sm font-medium">
          Password
        </label>
        <Input
          ref={fieldRef}
          id={fieldId}
          type="password"
          autoComplete="current-password"
          autoFocus
          value={password}
          disabled={busy}
          aria-invalid={failure?.kind === "wrong" ? true : undefined}
          aria-describedby={message ? `${fieldId}-error` : undefined}
          onChange={(e) => setPassword(e.target.value)}
          className="h-10"
        />
        {message ? (
          <p id={`${fieldId}-error`} role="alert" className="text-sm text-destructive">
            {message}
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

function TryAgain() {
  const [busy, setBusy] = useState(false)
  return (
    <Button
      variant="outline"
      className="h-10"
      disabled={busy}
      onClick={() => {
        setBusy(true)
        void probeAuth("required").finally(() => setBusy(false))
      }}
    >
      Try again
    </Button>
  )
}

export function BlockedPage({ where }: { where: string | null }) {
  return (
    <Shell title="This address is blocked">
      <p className="text-sm text-muted-foreground">
        dux refuses every request from the address you are connecting from. The
        block is an entry in <InlineCode>blocked_addresses</InlineCode>, in the{" "}
        <InlineCode>[server.auth]</InlineCode> section of{" "}
        {where ? <InlineCode>{where}</InlineCode> : <InlineCode>config.toml</InlineCode>}.
        Whoever runs dux can remove the address there and reload the config; dux
        also adds an address on its own after too many failed sign-ins.
      </p>
      <TryAgain />
    </Shell>
  )
}

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
      <TryAgain />
    </Shell>
  )
}
