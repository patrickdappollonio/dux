import { useState } from "react"

import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
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
import type { SessionView } from "@/lib/types"

// The form body is mounted only while the dialog is open and a session resolves.
// `provider` seeds from the session prop via a lazy initializer so there is no
// set-state-in-effect to seed the control on open.
function ChangeProviderForm({
  session,
  providers,
}: {
  session: SessionView
  providers: string[]
}) {
  const [provider, setProvider] = useState<string>(() => session.provider)
  const label = session.title ?? session.branch_name

  async function handleSave() {
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
    <DialogContent showCloseButton={false}>
      <DialogHeader>
        <DialogTitle>Change provider — {label}</DialogTitle>
        <DialogDescription>
          Pick the CLI this agent uses. The change takes effect the next time
          this agent launches; the running session keeps its current provider
          until you reconnect it. dux resumes the chosen provider&rsquo;s prior
          session on this worktree when one is available.
        </DialogDescription>
      </DialogHeader>

      <div className="grid gap-2">
        <label className="text-sm font-medium">Provider</label>
        <Select
          value={provider}
          onValueChange={(value) => setProvider(value ?? provider)}
        >
          <SelectTrigger className="w-full max-md:min-h-11">
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

      <DialogFooter>
        <Button variant="outline" onClick={closeChangeProvider}>
          Cancel
        </Button>
        <Button onClick={handleSave} disabled={provider === session.provider}>
          Change provider
        </Button>
      </DialogFooter>
    </DialogContent>
  )
}

// Swap which CLI an agent session uses, mirroring the TUI's
// `change-agent-provider`. The swap is persisted for the NEXT launch — it never
// kills or relaunches a running agent — so the copy says so. Provider names come
// from the bootstrap document's `available_providers` (the server's configured list), and
// the server re-validates the choice.
export function ChangeProviderDialog() {
  const { changeProviderTarget, spine, bootstrap } = useDux()
  const open = changeProviderTarget !== null
  const session = spine?.sessions.find((s) => s.id === changeProviderTarget)

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (!o) closeChangeProvider()
      }}
    >
      {open && session && (
        <ChangeProviderForm
          session={session}
          providers={bootstrap?.available_providers ?? []}
        />
      )}
    </Dialog>
  )
}
