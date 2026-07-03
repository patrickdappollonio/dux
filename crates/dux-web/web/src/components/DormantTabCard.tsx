import { Button } from "@/components/ui/button"
import { startDormantTab } from "@/lib/store"

// The center-pane surface for a DORMANT tab: a tab reopened after a restart whose
// provider process is not running. It renders WITHOUT opening the PTY socket (App
// gates this) because subscribing would force-launch the provider; only the "Start
// session" button launches it, via `startDormantTab`.
//
// The message is deliberately PROVIDER-AGNOSTIC: dux doesn't itself restore a tab's
// conversation after a restart. Launching may resume the provider's most-recent
// conversation in this worktree (when it's the sole live tab of that provider) or
// start fresh; either way the provider's own history command can browse prior ones,
// and different CLIs name that command differently, so we don't name one.
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
          dux doesn&rsquo;t restore a tab&rsquo;s conversation after a restart, but
          your CLI likely still has it: start it here (it may pick up where it left
          off), or use the provider&rsquo;s own command to browse and choose a
          previous conversation.
        </p>
      </div>
      <Button onClick={() => startDormantTab(sessionId, tabId)}>
        Start session
      </Button>
    </div>
  )
}
