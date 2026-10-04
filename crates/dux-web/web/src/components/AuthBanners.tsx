import { useState, useSyncExternalStore } from "react"
import { KeyRound, ShieldAlert } from "lucide-react"

import { Button } from "@/components/ui/button"
import { InlineCode } from "@/components/ui/inline-code"
import { postDismissNoAuthWarning, type AuthStatus } from "@/lib/authApi"
import { noteAuthStatus } from "@/lib/authGate"
import { notifyError, notifySuccess } from "@/lib/notify"
import { chip, prose } from "@/lib/prose"
import { openCustomizeWebapp } from "@/lib/store"

// The two auth banners an open page can carry, stacked over the top of
// whatever shell is up. Fixed rather than in flow, because every shell sizes
// itself to the viewport; dismissable, because a banner nobody can get rid of
// is a banner people learn to resent rather than read.
//
// "Dismiss" hides a banner for this page load only and writes nothing, so the
// no-password warning comes back on the next load. That is deliberate: it is
// meant to be annoying until the owner either sets a password or says, once,
// that they mean it ("Don't show again", which writes
// `disable_no_auth_warning = true`).

const dismissed = new Set<"no_auth" | "weak">()
const listeners = new Set<() => void>()

function dismiss(which: "no_auth" | "weak"): void {
  dismissed.add(which)
  for (const l of [...listeners]) l()
}

function subscribe(l: () => void): () => void {
  listeners.add(l)
  return () => listeners.delete(l)
}

// A version counter for `useSyncExternalStore`: the set itself is mutated in
// place, so its identity says nothing.
let version = 0
listeners.add(() => {
  version++
})

/// Test seam: forget every dismissal.
export function resetBannerDismissalsForTests(): void {
  dismissed.clear()
  version++
}

const BANNER_BUTTON = "max-md:min-h-10"

function NoPasswordBanner({ status }: { status: AuthStatus }) {
  const [busy, setBusy] = useState(false)
  const never = async () => {
    setBusy(true)
    try {
      await postDismissNoAuthWarning()
      noteAuthStatus({ ...status, no_auth_warning: false })
      notifySuccess(
        prose`dux will not show the no-password warning again. Set ${chip("disable_no_auth_warning")} back to false in config.toml to bring it back.`,
      )
    } catch (e) {
      notifyError(e instanceof Error ? e.message : "Could not save that choice.")
    } finally {
      setBusy(false)
    }
  }
  return (
    <div
      role="alert"
      className="flex flex-col gap-2 border-b border-destructive/60 bg-destructive/20 px-4 py-2.5 text-sm text-foreground md:flex-row md:items-center md:gap-4"
    >
      <ShieldAlert className="hidden size-5 shrink-0 text-destructive md:block" aria-hidden />
      <p className="min-w-0 flex-1">
        <strong className="font-semibold text-destructive">No password: anyone who can reach this address can use dux.</strong>{" "}
        They can run commands as you, read and change your files, and drive every
        agent.{" "}
        {status.can_set_first_password ? "Set one in Preferences, or run " : "Set one by running "}
        <InlineCode>dux config set server.auth.password</InlineCode>{" "}
        on the machine dux runs on.
      </p>
      <div className="flex shrink-0 gap-2">
        <Button
          variant="outline"
          size="sm"
          className={BANNER_BUTTON}
          onClick={() => dismiss("no_auth")}
        >
          Dismiss
        </Button>
        <Button
          variant="outline"
          size="sm"
          disabled={busy}
          className={BANNER_BUTTON}
          onClick={() => void never()}
        >
          Don&apos;t show again
        </Button>
      </div>
    </div>
  )
}

function WeakPasswordBanner() {
  return (
    <div
      role="status"
      className="flex flex-col gap-2 border-b border-border bg-card px-4 py-2.5 text-sm text-card-foreground md:flex-row md:items-center md:gap-4"
    >
      <KeyRound className="hidden size-5 shrink-0 text-destructive md:block" aria-hidden />
      <p className="min-w-0 flex-1">
        <strong className="font-semibold">The dux password is weaker than the minimum it asks for.</strong>{" "}
        It still signs you in, but anyone who sees your config file can try to
        guess it offline, and blocking addresses does not slow that down. Change it
        to a long, unique one.
      </p>
      <div className="flex shrink-0 gap-2">
        <Button
          variant="outline"
          size="sm"
          className={BANNER_BUTTON}
          onClick={() => dismiss("weak")}
        >
          Dismiss
        </Button>
        <Button
          variant="outline"
          size="sm"
          className={BANNER_BUTTON}
          onClick={() => openCustomizeWebapp()}
        >
          Change it in Preferences…
        </Button>
      </div>
    </div>
  )
}

export function AuthBanners({ status }: { status: AuthStatus | null }) {
  useSyncExternalStore(
    subscribe,
    () => version,
    () => version,
  )
  if (status === null) return null
  const showNoAuth = status.no_auth_warning && !dismissed.has("no_auth")
  const showWeak = status.weak_password && !dismissed.has("weak")
  if (!showNoAuth && !showWeak) return null
  return (
    // Under the dialogs (z-50), so a dialog opened from a banner sits on top.
    <div
      className="fixed inset-x-0 top-0 z-40 flex flex-col bg-background shadow-md"
      style={{ paddingTop: "env(safe-area-inset-top)" }}
    >
      {showNoAuth ? <NoPasswordBanner status={status} /> : null}
      {showWeak ? <WeakPasswordBanner /> : null}
    </div>
  )
}
