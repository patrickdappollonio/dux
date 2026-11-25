import { useState } from "react"

import { Modal, useHeldWhileClosed, useModalLifetime } from "@/components/modal"
import { FormGroup } from "@/components/ui/form-group"
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
import { sessionLabel, workspacePlace } from "@/lib/agentWorkspace"

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
  // A standalone agent works in the user's folder, not a worktree.
  const place = session ? workspacePlace(session.workspace) : "worktree"
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
          <span>
            Choose which CLI this agent&rsquo;s first tab runs. Its other tabs keep their own.
            Nothing changes right now: what is running keeps its current provider until you stop
            it and start it again.
          </span>
          <br />
          <br />
          <span>
            The next time it starts, it runs the provider you pick here. If that provider has
            worked in this {place} before and can pick up where it left off, dux continues that
            conversation, unless another tab of this agent is already running the same provider.
            Otherwise it starts a new one.
          </span>
        </>
      }
      submit={{
        label: "Change provider",
        tone: "primary",
        onSelect: () => void handleSave(),
        disabledReason: unchanged ? "Pick a provider other than the one this agent uses." : null,
      }}
    >
      <FormGroup layout="stacked" control="single" spokenName="Provider">
        {(field) => (
          <Select value={provider} onValueChange={(value) => setProvider(value ?? provider)}>
            <SelectTrigger {...field} className="w-full">
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
        )}
      </FormGroup>
    </Modal>
  )
}
