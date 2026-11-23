import { useState } from "react"

import { Modal, useHeldWhileClosed, useModalLifetime } from "@/components/modal"
import { InlineCode } from "@/components/ui/inline-code"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  changeAgentProvider,
  closeChangeProvider,
  useDux,
} from "@/lib/store"
import { sessionLabel } from "@/lib/agentWorkspace"

// Swap which CLI an agent session uses, mirroring the TUI's
// `change-agent-provider`. The swap is persisted for the NEXT launch and never
// kills or relaunches a running agent, so the copy says so. Provider names come
// from the bootstrap document's `available_providers` (the server's configured list), and
// the server re-validates the choice.
export function ChangeProviderDialog() {
  const { changeProviderTarget } = useDux()
  const open = changeProviderTarget !== null
  const life = useModalLifetime(open)
  // Keyed by the open, so the choice starts from the agent's provider each time.
  return life.mounted ? (
    <ChangeProviderForm key={life.epoch} open={open} onClosed={life.onClosed} />
  ) : null
}

function ChangeProviderForm({ open, onClosed }: { open: boolean; onClosed: () => void }) {
  const { changeProviderTarget, spine, bootstrap } = useDux()
  // The agent this open is about, kept while the close animates.
  const [targetId] = useState(changeProviderTarget)
  // The agent as it was when the close began, kept while the close animates,
  // so a change that lands as it closes does not disable its button.
  const session = useHeldWhileClosed(
    open,
    spine?.sessions.find((s) => s.id === targetId),
  )
  const providers = bootstrap?.available_providers ?? []
  const [provider, setProvider] = useState<string>(() => session?.provider ?? "")
  const unchanged = session === undefined || provider === session.provider

  async function handleSave() {
    if (session === undefined) return
    // No change → just close. Otherwise close only once the change is accepted, so
    // a rejected/invalid provider keeps the dialog open (error toasted) instead of
    // dismissing over a change that never applied.
    if (provider === session.provider) {
      closeChangeProvider()
      return
    }
    if (await changeAgentProvider(session.id, provider)) closeChangeProvider()
  }

  return (
    <Modal
      kind="form"
      open={open}
      // An agent removed while the dialog is open closes it; a rejected save,
      // by contrast, deliberately keeps it open.
      vanished={open && session === undefined}
      onClose={closeChangeProvider}
      row={targetId === null ? null : { sessionId: targetId }}
      onClosed={onClosed}
      title={
        session ? (
          <>
            Change provider: <InlineCode>{sessionLabel(session)}</InlineCode>
          </>
        ) : (
          "Change provider"
        )
      }
      description={
        <>
          Pick the CLI this agent uses. The change takes effect the next time this agent
          launches; the running session keeps its current provider until you reconnect it.
          dux resumes the chosen provider&rsquo;s prior session on this worktree when one is
          available and no other tab of that provider is already running.
        </>
      }
      submit={{
        label: "Change provider",
        tone: "primary",
        onSelect: () => void handleSave(),
        disabledReason: unchanged ? "Pick a provider other than the one this agent uses." : null,
      }}
    >
      <div className="grid gap-2">
        <label className="text-sm font-medium">Provider</label>
        <Select value={provider} onValueChange={(value) => setProvider(value ?? provider)}>
          <SelectTrigger className="w-full">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {providers.map((p) => (
              <SelectItem key={p} value={p}>
                {p}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
    </Modal>
  )
}
