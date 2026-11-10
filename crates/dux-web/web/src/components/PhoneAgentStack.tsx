import { Suspense, type ReactNode } from "react"

import { ChunkBoundary } from "@/components/ChunkBoundary"
import { DormantTabSurface } from "@/components/DormantTabSurface"
import { LazyTerminalStack } from "@/components/LazyTerminalPane"
import type { StackPartSpec } from "@/components/TerminalStack"
import { cn } from "@/lib/utils"

import type { AgentStackModel } from "./agentStackModel"

// An agent's stack on a phone: the agent or its terminal, never both. The part
// that is not on screen stays mounted in a box the size of the one that is,
// clipped, so its program keeps a real grid and switching back costs no
// reattach. The part on screen is the one typing goes to.
export function PhoneAgentStack({
  model,
  terminalEpoch,
  overlay,
}: {
  model: AgentStackModel
  terminalEpoch: number
  overlay: ReactNode
}) {
  const { sessionId, parts: on } = model
  const stackParts: StackPartSpec[] = []
  if (on.agentLiveSocket) {
    stackParts.push({
      slot: "agent",
      // A reconnect replaces the agent's process, so its part re-attaches.
      instance: `${model.tabId}:${terminalEpoch}`,
      kind: "agent",
      id: model.tabId,
      sessionId,
      slotTabId: model.slotTabId,
      onScreen: on.agent,
    })
  }
  if (model.activeTerminal !== null) {
    stackParts.push({
      slot: "dock",
      instance: model.activeTerminal,
      kind: "terminal",
      id: model.activeTerminal,
      owner: { kind: "session", sessionId },
      onScreen: on.dock,
    })
  }
  // Only on screen: a card has no connection to keep alive out of sight.
  const card =
    model.tab !== undefined && on.agent && model.agentCard !== null ? (
      <DormantTabSurface
        sessionId={sessionId}
        tabId={model.tab.id}
        provider={model.tab.provider}
        lastRunFailed={model.tab.last_run_failed === true}
        lastRunVerdict={model.tab.last_run_verdict}
      />
    ) : null

  return (
    <ChunkBoundary>
      <Suspense fallback={null}>
        <LazyTerminalStack
          parts={stackParts}
          overlay={overlay}
          typingTarget={model.typingPtyId}
          // A phone has one part on screen and it is already where typing goes;
          // moving between the parts is the switch's job.
          onChooseTypingTarget={() => {}}
          arrange={(rendered) => (
            <div className="relative flex min-h-0 w-full flex-1 flex-col overflow-hidden">
              <PhonePartBox onScreen={on.agent} testId="stack-agent-box">
                {rendered.agent ?? card}
              </PhonePartBox>
              <PhonePartBox onScreen={on.dock} testId="stack-dock-box">
                {rendered.dock ?? null}
              </PhonePartBox>
            </div>
          )}
        />
      </Suspense>
    </ChunkBoundary>
  )
}

// One part's box. On screen it fills the stack; off screen it keeps that same
// box just below the stack's edge, where the stack clips it.
function PhonePartBox({
  onScreen,
  testId,
  children,
}: {
  onScreen: boolean
  testId: string
  children: ReactNode
}) {
  return (
    <div
      data-testid={testId}
      data-on-screen={onScreen}
      className={cn(
        "flex w-full flex-col",
        onScreen ? "relative min-h-0 flex-1" : "absolute inset-x-0 top-full h-full",
      )}
    >
      {children}
    </div>
  )
}
