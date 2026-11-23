import { DormantTabCard } from "@/components/DormantTabCard"
import { NotRunningCard } from "@/components/NotRunningCard"
import { Welcome } from "@/components/Welcome"
import { startDormantTab } from "@/lib/store"
import { chip, prose, renderProse } from "@/lib/prose"
import type { TabRunVerdict } from "@/lib/types"

// What a dormant tab looks like, in one decision both shells read:
//   - simply NOT RUNNING is the workspace at rest, so it gets the idle screen
//     the terminal UI paints, plus the one act that starts the tab;
//   - a LAST RUN THAT ENDED BADLY gets the card: what the run did, how long ago,
//     its last output, and the same button.
export function DormantTabSurface({
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
  if (lastRunFailed) {
    return (
      <DormantTabCard
        sessionId={sessionId}
        tabId={tabId}
        provider={provider}
        lastRunFailed
        lastRunVerdict={lastRunVerdict}
      />
    )
  }
  return (
    <Welcome
      action={
        // The idle screen's logo is the card's icon, so the card draws none.
        <div className="mt-6">
          <NotRunningCard
            icon="none"
            title={renderProse(prose`This ${chip(provider)} tab isn’t running.`)}
            details={
              <>
                Starting it picks up this provider&rsquo;s most recent
                conversation in this worktree, unless another tab of the same
                provider is already running.
              </>
            }
            start={{
              sessionId,
              onStart: () => startDormantTab(sessionId, tabId),
              label: "Start session",
            }}
          />
        </div>
      }
    />
  )
}
