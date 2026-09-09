import { Button } from "@/components/ui/button"
import { DOCS_AGENT_TABS_RESUME } from "@/lib/docs"
import { startDormantTab } from "@/lib/store"
import { endingSentence, genericEndingSentence } from "@/lib/tabVerdict"
import type { TabRunVerdict } from "@/lib/types"

// The center-pane surface for a dormant tab waiting to be asked: an extra tab
// with no process, or any tab whose last run ended badly. It renders without
// opening the PTY socket, which `dormantTabNeedsCard` gates, because subscribing
// would force-launch the provider; only the start button launches it. An agent's
// own first tab does not come here after a plain restart or stop.
//
// A tab whose last run ended badly gets one extra sentence, and its last lines
// under it when the run left any: the case this exists for is a provider that
// printed the answer on its way out. The words come from `lib/tabVerdict.ts`, a
// port of `dux_core::tab_verdict`, so the terminal UI's card says the same, and
// they are neutral about blame, because a non-zero exit is often the user
// quitting the CLI. Everything else is the same for both, the way forward being
// the same.
//
// The message is provider-agnostic and states the rule: launching resumes the
// provider's most-recent conversation in this worktree when this is the sole
// live-or-launching tab of that provider, and starts fresh otherwise. CLIs name
// their history commands differently, so none is named here.
export function DormantTabCard({
  sessionId,
  tabId,
  provider,
  lastRunFailed,
  lastRunVerdict,
}: {
  sessionId: string
  tabId: string
  provider: string
  lastRunFailed?: boolean
  lastRunVerdict?: TabRunVerdict | null
}) {
  const excerpt = lastRunVerdict?.excerpt ?? []
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
            {lastRunVerdict
              ? endingSentence(lastRunVerdict)
              : genericEndingSentence()}
          </p>
        ) : null}
        {lastRunFailed && excerpt.length > 0 ? (
          <div className="space-y-1 text-left">
            <p className="text-xs font-medium text-muted-foreground">
              Last output
            </p>
            <pre className="max-h-40 overflow-auto whitespace-pre-wrap break-words rounded-md border border-border/60 bg-muted/40 px-2 py-1.5 text-left font-mono text-xs text-muted-foreground/80">
              {excerpt.join("\n")}
            </pre>
          </div>
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
            How resume works&nbsp;→
          </a>
        </p>
      </div>
      {/* The touch floor, at the take-over card's height idiom: that card is the
        * nearest analogue here, a full-pane card with one primary act, so
        * matching it keeps the two the same size under a finger. */}
      <Button
        onClick={() => startDormantTab(sessionId, tabId)}
        className="max-md:min-h-11"
      >
        Start session
      </Button>
    </div>
  )
}
