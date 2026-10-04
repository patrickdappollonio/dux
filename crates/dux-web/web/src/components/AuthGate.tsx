import type { ReactNode } from "react"

import { AuthBanners } from "@/components/AuthBanners"
import { BlockedPage, BrokenPage, LoginPage } from "@/components/LoginPage"
import { assertNever } from "@/lib/assertNever"
import { useAuthPhase } from "@/lib/authGate"

// The root of the page: the app when the sign-in gate is open, and the gate's
// own page otherwise. The app is unmounted while signed out rather than hidden
// under the login: what must survive a sign-out (the store, the URL, the
// editor's drafts) lives in page memory outside React, and a hidden terminal
// would go on measuring itself and sending sizes nobody can see.
export function AuthGate({ children }: { children: ReactNode }) {
  const phase = useAuthPhase()
  switch (phase.kind) {
    case "checking":
      // One round trip; the app background rather than a spinner, so a fast
      // answer does not flash anything.
      return <div className="min-h-svh bg-background" aria-busy="true" />
    case "open":
      return (
        <>
          {children}
          <AuthBanners status={phase.status} />
        </>
      )
    case "signed_out":
      return <LoginPage status={phase.status} reason={phase.reason} />
    case "blocked":
      return <BlockedPage where={phase.where} />
    case "broken":
      return <BrokenPage detail={phase.detail} />
    default:
      return assertNever(phase)
  }
}
