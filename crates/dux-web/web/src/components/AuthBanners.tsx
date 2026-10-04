import { useEffect, useLayoutEffect, useRef, useState } from "react"
import { KeyRound, ShieldAlert } from "lucide-react"

import { Button } from "@/components/ui/button"
import { InlineCode } from "@/components/ui/inline-code"
import { postDismissNoAuthWarning } from "@/lib/authActions"
import type { AuthStatus } from "@/lib/authApi"
import { refreshAuthStatus } from "@/lib/authGate"
import { dismissBanner, useDismissedBanners } from "@/lib/bannerDismissals"
import { notifyError, notifySuccess } from "@/lib/notify"
import { chip, prose } from "@/lib/prose"
import { openCustomizeWebapp } from "@/lib/store"

// The two auth banners an open page can carry, in the page's flow above the
// shell rather than over it, so the header, the cog and the phone flap stay
// where they are and reachable. The shells size themselves to the viewport
// minus `--dux-app-top`, which this stack publishes as its own height.
//
// "Dismiss" hides a banner for this page load only and writes nothing, so the
// no-password warning comes back on the next load. That is deliberate: it is
// meant to be annoying until the owner either sets a password or says, once,
// that they mean it ("Don't show again", which writes
// `disable_no_auth_warning = true`).

// On a coarse pointer every banner button meets the 40px floor whatever the
// width; with a mouse the default height stands, because the only neighbours
// are the banner's own text and the other button, a gap-2 away.
const BANNER_BUTTON = "pointer-coarse:min-h-11"

const APP_TOP_VAR = "--dux-app-top"
const APP_SAFE_TOP_VAR = "--dux-app-safe-top"

function NoPasswordBanner({ canSetFirst }: { canSetFirst: boolean }) {
  const [busy, setBusy] = useState(false)
  const never = async () => {
    setBusy(true)
    try {
      await postDismissNoAuthWarning()
      dismissBanner("no_auth")
      notifySuccess(
        prose`dux will not show the no-password warning again. Set ${chip("disable_no_auth_warning")} back to false in config.toml to bring it back.`,
      )
      // What the server says now, rather than a patched copy of what it said.
      void refreshAuthStatus()
    } catch (e) {
      notifyError(e instanceof Error ? e.message : "Could not save that choice.")
    } finally {
      setBusy(false)
    }
  }
  return (
    <div
      role="alert"
      data-testid="no-auth-banner"
      className="flex flex-col gap-2 border-b border-destructive/60 bg-destructive/20 px-4 py-2.5 text-sm text-foreground md:flex-row md:items-center md:gap-4"
    >
      <ShieldAlert className="hidden size-5 shrink-0 text-destructive md:block" aria-hidden />
      <p className="min-w-0 flex-1">
        <strong className="font-semibold text-destructive">
          No password: anyone who can reach this address can use dux.
        </strong>{" "}
        They can run commands as you, read and change your files, and drive every
        agent. {canSetFirst ? "Set one in Preferences, or run " : "Set one by running "}
        <InlineCode>dux config set server.auth.password</InlineCode> on the machine
        dux runs on.
      </p>
      <div className="flex shrink-0 gap-2">
        <Button
          variant="outline"
          size="sm"
          className={BANNER_BUTTON}
          onClick={() => dismissBanner("no_auth")}
        >
          Dismiss
        </Button>
        <Button
          variant="outline"
          size="sm"
          disabled={busy}
          data-testid="no-auth-banner-never"
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
      data-testid="weak-password-banner"
      className="flex flex-col gap-2 border-b border-border bg-card px-4 py-2.5 text-sm text-card-foreground md:flex-row md:items-center md:gap-4"
    >
      <KeyRound className="hidden size-5 shrink-0 text-destructive md:block" aria-hidden />
      <p className="min-w-0 flex-1">
        <strong className="font-semibold">
          The dux password is weaker than the minimum it asks for.
        </strong>{" "}
        It still signs you in, but anyone who sees your config file can try to
        guess it offline, and blocking addresses does not slow that down. Change it
        to a long, unique one.
      </p>
      <div className="flex shrink-0 gap-2">
        <Button
          variant="outline"
          size="sm"
          className={BANNER_BUTTON}
          onClick={() => dismissBanner("weak")}
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

// Publish the stack's height for the shells to subtract, and take it back when
// the stack goes. While it shows, the stack clears the notch itself, so the
// phone shell's own top inset is turned off (`--dux-app-safe-top`) rather than
// paid twice.
function usePublishedHeight(ref: React.RefObject<HTMLDivElement | null>, shown: boolean): void {
  useLayoutEffect(() => {
    const root = document.documentElement
    const clear = () => {
      root.style.removeProperty(APP_TOP_VAR)
      root.style.removeProperty(APP_SAFE_TOP_VAR)
    }
    const el = ref.current
    if (!shown || el === null) {
      clear()
      return
    }
    const publish = () => root.style.setProperty(APP_TOP_VAR, `${el.offsetHeight}px`)
    root.style.setProperty(APP_SAFE_TOP_VAR, "0px")
    publish()
    if (typeof ResizeObserver === "undefined") return clear
    const observer = new ResizeObserver(publish)
    observer.observe(el)
    return () => {
      observer.disconnect()
      clear()
    }
  }, [ref, shown])
}

export function AuthBanners({ status }: { status: AuthStatus | null }) {
  const dismissed = useDismissedBanners()
  const ref = useRef<HTMLDivElement>(null)
  // Shown means read again: what the server says now, not what it said when
  // the page opened.
  useEffect(() => {
    void refreshAuthStatus()
  }, [])
  const showNoAuth = status !== null && status.no_auth_warning && !dismissed.has("no_auth")
  const showWeak = status !== null && status.weak_password && !dismissed.has("weak")
  const shown = showNoAuth || showWeak
  usePublishedHeight(ref, shown)
  if (!shown) return null
  return (
    <div
      ref={ref}
      data-testid="auth-banners"
      className="relative z-40 flex flex-col bg-background"
      style={{ paddingTop: "env(safe-area-inset-top)" }}
    >
      {showNoAuth ? <NoPasswordBanner canSetFirst={status?.can_set_first_password ?? false} /> : null}
      {showWeak ? <WeakPasswordBanner /> : null}
    </div>
  )
}
