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
  createAgent,
  setCreateAgentDraft,
  toggleCreateAgentRandomize,
  useDux,
} from "@/lib/store"

// The new-agent dialog mirrors the TUI prompt: the input filters characters live
// (spaces -> dashes, disallowed chars dropped), and a "Use randomized pet name"
// checkbox fills the input with a server-generated name when toggled on (and
// clears it on toggle off only if the text is still that generated name). All of
// that state lives in the store (like the commit draft), so the server's
// generated-name reply fills the input through an event callback — never a
// set-state-in-effect.
export function CreateAgentDialog() {
  const {
    createAgentTarget,
    createAgentDraft,
    createAgentRandomize,
    createAgentNamePending,
    viewModel,
  } = useDux()
  const open = createAgentTarget !== null
  const project = viewModel?.projects.find((p) => p.id === createAgentTarget)
  const projectName = project?.name ?? "project"

  // A generation request is in flight: show the spinner and disable the input
  // so a late reply can never clobber text typed in the meantime. Tracked
  // explicitly in the store — manually clearing the input does NOT fake this.
  const generating = createAgentNamePending
  // The Create button is gated only when there's a non-empty invalid name (e.g.
  // a trailing slash mid-typing). Empty is allowed: the server auto-generates,
  // the equivalent outcome to the TUI's generate-a-pet-name path.
  const invalid = createAgentDraft !== "" && !isValidAgentName(createAgentDraft)

  function handleCreate() {
    if (!createAgentTarget || invalid) return
    createAgent(createAgentTarget, createAgentDraft.trim())
    closeCreateAgent()
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
          <DialogTitle>New agent in {projectName}</DialogTitle>
          <DialogDescription>
            Creates a git worktree + branch and launches the agent. Tick
            &ldquo;Use randomized pet name&rdquo; to autofill a generated name.
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
                handleCreate()
              }
            }}
            placeholder="Branch name (optional)"
            aria-invalid={invalid}
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
          <Button onClick={handleCreate} disabled={invalid}>
            Create agent
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
