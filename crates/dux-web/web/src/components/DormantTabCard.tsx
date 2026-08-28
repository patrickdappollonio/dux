import { Button } from "@/components/ui/button"
import { DOCS_AGENT_TABS_RESUME } from "@/lib/docs"
import { startDormantTab } from "@/lib/store"

// The center-pane surface for a DORMANT tab: a tab reopened after a restart whose
// provider process is not running. It renders WITHOUT opening the PTY socket (App
// gates this) because subscribing would force-launch the provider; only the "Start
// session" button launches it, via `startDormantTab`.
//
// The message is deliberately PROVIDER-AGNOSTIC and states the actual rule:
// launching resumes the provider's most-recent conversation in this worktree when
// this is the sole live-or-launching tab of that provider, and starts fresh
// otherwise. Older conversations are reachable through the provider's own history
// command, and different CLIs name that command differently, so we don't name one.
export function DormantTabCard({
  sessionId,
  tabId,
  provider,
}: {
  sessionId: string
  tabId: string
  provider: string
}) {
  return (
    <div className="flex h-full w-full select-none flex-col items-center justify-center gap-4 overflow-hidden px-6 text-center">
      <img
        src="/dux-logo.png"
        alt=""
        aria-hidden
        className="size-20 object-contain opacity-70"
      />
      <div className="max-w-md space-y-2">
        <p className="text-sm font-medium">
          This <span className="font-mono">{provider}</span> tab isn&rsquo;t running.
        </p>
        <p className="text-sm text-muted-foreground">
          Starting it picks up this provider&rsquo;s most recent conversation in
          this worktree, unless another tab of the same provider is already running
          or the provider can&rsquo;t resume, in which case it starts fresh. To reach
          an older conversation, use the provider&rsquo;s own history command.{" "}
          <a
            href={DOCS_AGENT_TABS_RESUME}
            target="_blank"
            rel="noopener noreferrer"
            className="text-primary underline underline-offset-2"
          >
            How resume works →
          </a>
        </p>
      </div>
      <Button onClick={() => startDormantTab(sessionId, tabId)}>
        Start session
      </Button>
    </div>
  )
}
