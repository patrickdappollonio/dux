import { Button } from "@/components/ui/button"
import { DOCS_AGENT_TABS_RESUME } from "@/lib/docs"
import { startDormantTab } from "@/lib/store"

// The center-pane surface for a dormant tab that is waiting to be asked: an
// extra tab with no process, or any tab whose last run ended badly. It renders
// WITHOUT opening the PTY socket (`dormantTabNeedsCard` gates this) because
// subscribing would force-launch the provider; only the "Start session" button
// launches it, via `startDormantTab`. An agent's own first tab does not come
// here after a plain restart or stop: selecting the agent starts it.
//
// Two dormant tabs look identical without a word about WHY a press is needed,
// so a tab whose last run ended badly gets one extra sentence. It is deliberately
// neutral about blame: a non-zero exit is often the user quitting the CLI in a way
// it reports as an error, so the sentence says what dux observed and what dux
// therefore did not do, and never that anything crashed. Everything else on the
// card is the same for both, because the way forward is the same.
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
  lastRunFailed,
}: {
  sessionId: string
  tabId: string
  provider: string
  lastRunFailed?: boolean
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
        {lastRunFailed ? (
          <p className="text-sm text-muted-foreground">
            Its last run ended with an error or a non-zero exit, so dux
            didn&rsquo;t start it again on its own.
          </p>
        ) : null}
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
