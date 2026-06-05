import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { isValidAgentName, sanitizeAgentName } from "@/lib/agentName"
import {
  closeCreateAgent,
  setCreateAgentDraft,
  submitNameDialog,
  toggleCreateAgentRandomize,
  useDux,
} from "@/lib/store"

// The name dialog mirrors the TUI prompt and serves two modes — creating a fresh
// agent and forking an existing session — switched on the store's
// `createAgentTarget.kind`. The input filters characters live (spaces -> dashes,
// disallowed chars dropped), and a "Use randomized pet name" checkbox fills the
// input with a server-generated name when toggled on (and clears it on toggle
// off only if the text is still that generated name). All of that state lives in
// the store (like the commit draft), so the server's generated-name reply fills
// the input through an event callback — never a set-state-in-effect.
export function CreateAgentDialog() {
  const {
    createAgentTarget,
    createAgentDraft,
    createAgentRandomize,
    createAgentNamePending,
    viewModel,
  } = useDux()
  const open = createAgentTarget !== null
  const isFork = createAgentTarget?.kind === "fork"
  const project =
    createAgentTarget?.kind === "new"
      ? viewModel?.projects.find((p) => p.id === createAgentTarget.projectId)
      : undefined
  const forkSession =
    createAgentTarget?.kind === "fork"
      ? viewModel?.sessions.find((s) => s.id === createAgentTarget.sessionId)
      : undefined
  const projectName = project?.name ?? "project"
  const sourceLabel = forkSession?.title || forkSession?.branch_name || "agent"

  // A generation request is in flight: show the spinner and disable the input
  // so a late reply can never clobber text typed in the meantime. Tracked
  // explicitly in the store — manually clearing the input does NOT fake this.
  const generating = createAgentNamePending
  // For a NEW agent, empty is allowed: the server auto-generates a pet name.
  // For a FORK, a name is REQUIRED — the server rejects an empty fork — so the
  // button is also gated on emptiness. Either way, a non-empty invalid name
  // (e.g. a trailing slash mid-typing) disables the button.
  const empty = createAgentDraft.trim() === ""
  const invalidNonEmpty = !empty && !isValidAgentName(createAgentDraft)
  const disabled = invalidNonEmpty || (isFork && empty)

  function handleSubmit() {
    if (disabled) return
    submitNameDialog(createAgentDraft.trim())
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (!o) closeCreateAgent()
      }}
    >
      <DialogContent showCloseButton={false}>
        <DialogHeader>
          <DialogTitle>
            {isFork ? `Fork ${sourceLabel}` : `New agent in ${projectName}`}
          </DialogTitle>
          <DialogDescription>
            {isFork
              ? "Forks the agent into a new git worktree + branch (copying its current files) and launches a fresh session."
              : "Creates a git worktree + branch and launches the agent. Tick “Use randomized pet name” to autofill a generated name."}
          </DialogDescription>
        </DialogHeader>
        <div className="relative">
          <Input
            value={createAgentDraft}
            onChange={(e) => {
              const el = e.target
              const raw = el.value
              const caret = el.selectionStart ?? raw.length
              setCreateAgentDraft(raw)
              // When sanitization changes the string (space→dash keeps the
              // length; dropped chars shrink it), React re-renders the
              // controlled value and the browser parks the caret at the end —
              // a jump on every mid-string edit. Restore it adjusted for the
              // length delta so typing in the middle of a name stays put.
              const sanitized = sanitizeAgentName(raw)
              if (sanitized !== raw) {
                const next = Math.max(0, caret - (raw.length - sanitized.length))
                requestAnimationFrame(() => el.setSelectionRange(next, next))
              }
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault()
                handleSubmit()
              }
            }}
            placeholder={isFork ? "Branch name" : "Branch name (optional)"}
            aria-invalid={invalidNonEmpty}
            disabled={generating}
            autoFocus
          />
          {generating && (
            <span className="absolute right-3 top-1/2 -translate-y-1/2 animate-spin text-muted-foreground">
              ⠋
            </span>
          )}
        </div>
        <p className="text-xs text-muted-foreground">
          Letters, digits, dashes, underscores and slashes — becomes the branch
          name.
        </p>
        <div className="flex items-center gap-2">
          <Checkbox
            id="randomize-agent-name"
            checked={createAgentRandomize}
            onCheckedChange={toggleCreateAgentRandomize}
          />
          <label htmlFor="randomize-agent-name" className="text-sm">
            Use randomized pet name
          </label>
        </div>
        <div className="h-2" />
        <DialogFooter>
          <Button variant="outline" onClick={closeCreateAgent}>
            Cancel
          </Button>
          <Button onClick={handleSubmit} disabled={disabled}>
            {isFork ? "Fork agent" : "Create agent"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
