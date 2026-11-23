import { useSecondTicks } from "@/hooks/use-second-ticks"
import { NotRunningCard } from "@/components/NotRunningCard"
import { startDormantTab } from "@/lib/store"
import { chip, prose, renderProse } from "@/lib/prose"
import { genericEndingSentence, verdictSentenceNow } from "@/lib/tabVerdict"
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
  useSecondTicks(lastRunVerdict != null)
  return (
    <NotRunningCard
      icon="large"
      title={
        renderProse(prose`This ${chip(provider)} tab isn’t running.`)
      }
      subtitle={
        lastRunFailed
          ? lastRunVerdict
            ? verdictSentenceNow(lastRunVerdict)
            : genericEndingSentence()
          : null
      }
      output={lastRunFailed ? (lastRunVerdict?.excerpt ?? []) : []}
      details={
        <>
          Starting it picks up this provider&rsquo;s most recent conversation in
          this worktree, unless another tab of the same provider is already running
          or the provider can&rsquo;t resume, in which case it starts fresh. To reach
          an older conversation, use the provider&rsquo;s own history command.
        </>
      }
      start={{
        sessionId,
        onStart: () => startDormantTab(sessionId, tabId),
        label: "Start session",
      }}
    />
  )
}
