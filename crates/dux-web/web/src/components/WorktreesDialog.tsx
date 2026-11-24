import { useState, type ReactNode } from "react"
import { Copy, FileWarning, FolderGit2, FolderX, Trash2 } from "lucide-react"

import { Spinner } from "@/components/Spinner"
import { EllipsisMenuTrigger } from "@/components/EllipsisMenuTrigger"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import {
  Modal,
  ModalGroupHeading,
  useHeldWhileClosed,
  useModalLifetime,
} from "@/components/modal"
import { CheckboxRow } from "@/components/CheckboxRow"
import { InlineCode } from "@/components/ui/inline-code"
import { EmptyState } from "@/components/ui/empty"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
} from "@/components/ui/dropdown-menu"
import { Input } from "@/components/ui/input"
import { ScrollArea } from "@/components/ui/scroll-area"
import { ScrollList, ScrollListRows } from "@/components/ui/scroll-list"
import { INVALID_AGENT_NAME, isValidAgentName, sanitizeAgentName } from "@/lib/agentName"
import { branchCheckboxLabel, predatesSentence } from "@/lib/branchWarning"
import { clipboardWorktree } from "@/lib/flatClipboard"
import {
  leftoverBranchFateProse,
  leftoverDeleteBodyProse,
  leftoverDeleteTitle,
  leftoverKeptBranchProse,
} from "@/lib/leftoverFolder"
import { chip, type Prose, renderProse } from "@/lib/prose"
import {
  attachWorktree,
  closeAttachWorktree,
  closeDeleteLeftover,
  closeDeleteWorktree,
  deleteLeftoverFolder,
  deleteProjectWorktree,
  openDeleteLeftover,
  openDeleteWorktree,
  openNewAgentPicker,
  useDux,
} from "@/lib/store"
import type { LeftoverFolderView, ProjectWorktreeEntryView } from "@/lib/types"
import { sessionLabel } from "@/lib/agentWorkspace"

// The last path segment (the worktree directory's name) is what the TUI shows
// as the entry label. Falls back to the full path for a root-level path.
function pathTail(path: string): string {
  const parts = path.split("/").filter(Boolean)
  return parts.length > 0 ? parts[parts.length - 1] : path
}

// The row truncates its branch, so the hover tooltip gives the full name beside
// the path (plus any disabled reason for attached rows).
function rowTooltip(entry: ProjectWorktreeEntryView): ReactNode {
  return (
    <span className="flex flex-col gap-0.5">
      {entry.reason ? <span>{entry.reason}</span> : null}
      <span className="font-mono">{entry.branch_name}</span>
      <span>{entry.worktree_path}</span>
    </span>
  )
}

// The shared row body: the folder icon plus the worktree's branch over its path,
// with a marker when it holds uncommitted work. `heldBy` is the display name of
// the agent attached to it, which an attached row points at in place of a delete.
function WorktreeRowBody({
  entry,
  heldBy,
  inUse,
}: {
  entry: ProjectWorktreeEntryView
  heldBy?: string
  // The server's sentence for what is using a worktree, other than the agent
  // holding it.
  inUse?: string | null
}) {
  return (
    <>
      <FolderGit2 className="size-4 shrink-0 text-muted-foreground" />
      {/* The branch over the path, so neither competes for the row's width.
         The branch truncates; the path is what tells two worktrees apart, so
         it wraps and is always read in full. */}
      <div className="flex min-w-0 flex-1 flex-col">
        <span className="truncate font-mono text-sm">{entry.branch_name}</span>
        <span className="text-xs break-all text-muted-foreground">
          {entry.worktree_path}
        </span>
        {heldBy ? (
          <span className="truncate text-xs text-muted-foreground">
            Held by <InlineCode>{heldBy}</InlineCode>
          </span>
        ) : null}
        {inUse ? (
          <span className="truncate text-xs text-muted-foreground">{inUse}</span>
        ) : null}
      </div>
      {entry.dirty ? (
        <SimpleTooltip content="This worktree has changes that are not committed anywhere.">
          {/* The warning tone the toast layer also uses, paired
             with an icon so colour is never the only signal. On a phone it
             takes a line of its own under the path, in line with the text,
             and leaves the row's width to the path. */}
          <span className="flex shrink-0 items-center gap-1 text-xs text-dux-warning max-md:basis-full max-md:pl-[26px]">
            <FileWarning className="size-3.5" />
            Uncommitted changes
          </span>
        </SimpleTooltip>
      ) : null}
    </>
  )
}

// The confirmation for removing a worktree. The removal is forcible and there is
// no trash, so this names the branch and the full path, says what is lost, and
// says it cannot be undone; a dirty worktree gets its own sentence, because work
// that exists nowhere else is the whole reason to stop.
//
// The branch checkbox defaults on: a leftover branch is what makes recreating an
// agent under that name fail, and it is not a bigger step than the removal
// already being confirmed. The server still defaults to false, so a request that
// says nothing never deletes a branch.
function ConfirmDeleteWorktree() {
  const { deleteWorktreeTarget, attachWorktreeEntries } = useDux()
  const open = deleteWorktreeTarget !== null
  // The worktree this was opened on, kept while the close animates.
  const target = useHeldWhileClosed(open, deleteWorktreeTarget)
  // The answer, kept with the worktree it was given for: the component stays
  // mounted across opens, and a confirmation on another row starts at the
  // default however soon it follows the last one.
  const [answer, setAnswer] = useState<{ path: string; deleteBranch: boolean } | null>(null)
  const targetPath = target?.entry.worktree_path ?? null
  const deleteBranch = answer !== null && answer.path === targetPath ? answer.deleteBranch : true
  const setDeleteBranch = (next: boolean) => {
    if (targetPath !== null) setAnswer({ path: targetPath, deleteBranch: next })
  }
  // The listing is refetched after every delete, so it is a live truth this
  // dialog can close itself on: a worktree that left the list (removed from
  // another tab, adopted by an agent) is no longer this dialog's subject.
  const stillListed =
    deleteWorktreeTarget !== null &&
    attachWorktreeEntries.some(
      (e) =>
        e.worktree_path === deleteWorktreeTarget.entry.worktree_path &&
        e.adoptable,
    )
  const entry = target?.entry
  // A detached worktree has no branch, so there is no choice to offer and the
  // request must not ask for one.
  const branch = entry?.branch ?? null

  function handleConfirm() {
    if (!deleteWorktreeTarget) return
    deleteProjectWorktree(
      deleteWorktreeTarget.projectId,
      deleteWorktreeTarget.entry.worktree_path,
      branch !== null && deleteBranch,
    )
    closeDeleteWorktree()
  }

  return (
    <Modal
      kind="confirm"
      open={open}
      vanished={open && !stillListed}
      onClose={closeDeleteWorktree}
      onClosed={() => setAnswer(null)}
      title={
        <>
          Delete the worktree for <InlineCode>{entry?.branch_name}</InlineCode>?
        </>
      }
      cancelLabel="Cancel"
      confirm={{ label: "Delete worktree", tone: "destructive", onSelect: handleConfirm }}
    >
      <div
        data-testid="delete-worktree-confirm"
        className="grid gap-2 text-sm text-destructive"
      >
        <p>
          <InlineCode>{entry?.worktree_path}</InlineCode> will be
          removed from disk. This action cannot be undone: dux has no trash and
          removes the directory forcibly.
        </p>
        {entry?.dirty ? (
          <p>
            This worktree has uncommitted changes, and they go with it. Nothing
            in there that is not committed exists anywhere else.
          </p>
        ) : null}
        {branch === null ? (
          <p className="text-muted-foreground">
            This worktree is not on a branch, so there is no branch to keep or
            delete. Only the working directory is removed.
          </p>
        ) : deleteBranch ? (
          <p>
            The branch <InlineCode>{branch}</InlineCode> will be
            deleted with it, forcibly. Any commits on it that are not merged
            anywhere else go too.
          </p>
        ) : (
          <p className="text-muted-foreground">
            The branch <InlineCode>{branch}</InlineCode> is kept.
            Only the working directory is removed.
          </p>
        )}
      </div>
      {branch !== null ? (
        <CheckboxRow
          id="delete-worktree-branch"
          checked={deleteBranch}
          onCheckedChange={setDeleteBranch}
        >
          Also delete the branch{" "}
          <InlineCode>{branch}</InlineCode>
        </CheckboxRow>
      ) : null}
    </Modal>
  )
}

// The kept branches a left-over row names, every one a chip.
function keptBranchesLine(branches: string[]): Prose {
  if (branches.length === 1) return ["Kept branch ", chip(branches[0])]
  return [
    "Kept branches ",
    ...branches.flatMap((branch, index): Prose =>
      index === 0 ? [chip(branch)] : [" and ", chip(branch)],
    ),
  ]
}

// Whether the row the delete dialog was opened on is still the row listed: the
// same folder, still deletable, with the same branches to delete, the same
// branches kept for the same reasons and the same folder state, since the
// dialog's sentences and its request are built from those.
function sameLeftover(listed: LeftoverFolderView, opened: LeftoverFolderView): boolean {
  return (
    listed.path === opened.path &&
    !listed.in_use &&
    listed.offers_delete === opened.offers_delete &&
    listed.folder_present === opened.folder_present &&
    listed.branches.length === opened.branches.length &&
    listed.branches.every((branch, index) => branch === opened.branches[index]) &&
    listed.undeletable.length === opened.undeletable.length &&
    listed.undeletable.every(
      (branch, index) =>
        branch.name === opened.undeletable[index].name &&
        branch.why === opened.undeletable[index].why,
    )
  )
}

// A left-over folder row: the folder's name over its full path, the branches
// kept with it, and what is using it now when something is. Once the folder
// itself is gone the row says so, and its delete is about the branches. A row
// with nothing a delete can remove offers no delete, and says of each branch
// it keeps why, and how to delete it yourself.
function LeftoverRow({
  projectId,
  folder,
}: {
  projectId: string
  folder: LeftoverFolderView
}) {
  const kept = [
    ...folder.branches,
    ...folder.undeletable.map((branch) => branch.name),
  ]
  return (
    <div className="flex min-h-11 w-full items-center gap-1 pr-1 hover:bg-accent/60 md:min-h-0">
      <div className="flex min-w-0 flex-1 items-center gap-2.5 px-3 py-2">
        <FolderX className="size-4 shrink-0 text-muted-foreground" />
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="truncate font-mono text-sm">{pathTail(folder.path)}</span>
          <span className="text-xs break-all text-muted-foreground">
            {folder.path}
          </span>
          {folder.folder_present ? null : (
            <span className="truncate text-xs text-muted-foreground">
              {kept.length === 1
                ? "Folder already removed; branch kept"
                : "Folder already removed; branches kept"}
            </span>
          )}
          {kept.length > 0 ? (
            <span className="truncate text-xs text-muted-foreground">
              {renderProse(keptBranchesLine(kept))}
            </span>
          ) : null}
          {folder.offers_delete
            ? null
            : folder.undeletable.map((branch) => (
                <span
                  key={branch.name}
                  data-testid="leftover-kept-branch"
                  className="text-xs text-muted-foreground"
                >
                  {renderProse(leftoverKeptBranchProse(branch))}
                </span>
              ))}
          {folder.in_use ? (
            <span className="truncate text-xs text-muted-foreground">
              {folder.in_use}
            </span>
          ) : null}
        </div>
      </div>
      <DropdownMenu>
        <EllipsisMenuTrigger placement="row" aria-label="Left-over folder actions" />
        <DropdownMenuContent align="end">
          <DropdownMenuItem onClick={() => clipboardWorktree(folder.path)}>
            <Copy />
            Copy path
          </DropdownMenuItem>
          {/* Absent while something uses the folder, since the server refuses
             the delete then, and when nothing on the row is a delete can
             remove; the row already says why in both cases. Neutral colour,
             per the row-menu convention. */}
          {folder.in_use || !folder.offers_delete ? null : (
            <DropdownMenuItem
              onClick={() => openDeleteLeftover(projectId, folder)}
            >
              <Trash2 />
              {folder.folder_present
                ? "Delete folder…"
                : folder.branches.length === 1
                  ? "Delete branch…"
                  : "Delete branches…"}
            </DropdownMenuItem>
          )}
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  )
}

// The confirmation for deleting a left-over folder. Like the worktree removal
// it is forcible with no trash, and it says why the last delete failed, since
// whatever kept writing there will make this one fail too. Its sentences are
// dux-core's, shared with the terminal UI (see lib/leftoverFolder.ts).
//
// With the folder already gone the delete is the kept branches and nothing
// else, so there is no box: every branch a delete can remove goes with the
// request, and the confirm says so. A kept branch a delete cannot remove is
// never in the request or the box; the dialog says why it is kept instead.
//
// The branch box is ticked to begin with for a branch dux created, and for a
// folder the manager's own removal left (null provenance), whose branch the
// user had already asked to delete; a branch that predates its agent starts
// unticked and carries the agent-delete dialog's warning for its provenance.
function ConfirmDeleteLeftoverFolder() {
  const { deleteLeftoverTarget, attachWorktreeLeftovers } = useDux()
  const open = deleteLeftoverTarget !== null
  // The folder this was opened on, kept while the close animates.
  const folder = useHeldWhileClosed(open, deleteLeftoverTarget)?.folder
  const provenance = folder?.branch_provenance ?? null
  const tickedByDefault = provenance === null || provenance === "created"
  // The user's answer, kept with the folder it was given for, so each
  // confirmation starts at the default for its own folder however soon it
  // follows the last one.
  const [answer, setAnswer] = useState<{ path: string; ticked: boolean } | null>(null)
  const choice = answer !== null && answer.path === folder?.path ? answer.ticked : null
  const folderPresent = folder?.folder_present ?? true
  const deleteBranches = !folderPresent || (choice ?? tickedByDefault)
  const stillListed =
    deleteLeftoverTarget !== null &&
    attachWorktreeLeftovers.some((f) =>
      sameLeftover(f, deleteLeftoverTarget.folder),
    )
  const branches = folder?.branches ?? []
  const undeletable = folder?.undeletable ?? []
  const warning =
    branches.length > 0 && provenance !== null
      ? predatesSentence(provenance, branches)
      : null

  function handleConfirm() {
    if (!deleteLeftoverTarget) return
    deleteLeftoverFolder(
      deleteLeftoverTarget.projectId,
      deleteLeftoverTarget.folder.path,
      deleteBranches ? branches : [],
    )
    closeDeleteLeftover()
  }

  return (
    <Modal
      kind="confirm"
      open={open}
      vanished={open && !stillListed}
      onClose={closeDeleteLeftover}
      onClosed={() => setAnswer(null)}
      title={leftoverDeleteTitle(folderPresent, branches.length)}
      cancelLabel="Cancel"
      confirm={{
        label: folderPresent
          ? "Delete folder"
          : branches.length === 1
            ? "Delete branch"
            : "Delete branches",
        tone: "destructive",
        onSelect: handleConfirm,
      }}
    >
      <div
        data-testid="delete-leftover-confirm"
        className="grid gap-2 text-sm text-destructive"
      >
        <p>
          {renderProse(
            leftoverDeleteBodyProse(
              folder?.path ?? "",
              folderPresent,
              folder?.left_by !== "manager",
            ),
          )}
        </p>
        {branches.length > 0 ? (
          <p
            data-testid="delete-leftover-branch-fate"
            className={deleteBranches ? undefined : "text-muted-foreground"}
          >
            {renderProse(
              leftoverBranchFateProse(branches, deleteBranches, folderPresent),
            )}
          </p>
        ) : null}
        {undeletable.map((branch) => (
          <p
            key={branch.name}
            data-testid="delete-leftover-kept-branch"
            className="text-muted-foreground"
          >
            {renderProse(leftoverKeptBranchProse(branch))}
          </p>
        ))}
      </div>
      {branches.length > 0 ? (
        <div className="grid gap-2">
          {folderPresent ? (
            <CheckboxRow
              id="delete-leftover-branches"
              checked={deleteBranches}
              onCheckedChange={(ticked) => {
                if (folder) setAnswer({ path: folder.path, ticked })
              }}
            >
              {renderProse(branchCheckboxLabel(branches))}
            </CheckboxRow>
          ) : null}
          {/* The danger sits in the sentence, never in a red checkbox, as in
             the agent-delete dialog. */}
          {warning !== null ? (
            <p
              data-testid="delete-leftover-branch-warning"
              className="text-sm text-destructive"
            >
              {renderProse(warning)}
            </p>
          ) : null}
        </div>
      ) : null}
    </Modal>
  )
}

// Keyed by the open, so its selection and its name start empty on each one.
function WorktreesBody({ open, onClosed }: { open: boolean; onClosed: () => void }) {
  const state = useDux()
  const { spine } = state
  // The project this open is about, and the listing as it was when the close
  // began: closing empties the store's copy. The store's target is followed
  // while open, because the listing it holds is that project's.
  const projectId = useHeldWhileClosed(open, state.attachWorktreeTarget) ?? ""
  const attachWorktreeEntries = useHeldWhileClosed(open, state.attachWorktreeEntries)
  const attachWorktreeLeftovers = useHeldWhileClosed(open, state.attachWorktreeLeftovers)
  const attachWorktreeLoading = useHeldWhileClosed(open, state.attachWorktreeLoading)
  const attachWorktreeFromPicker = useHeldWhileClosed(open, state.attachWorktreeFromPicker)
  const [selected, setSelected] = useState<string | null>(null)
  const [name, setName] = useState("")
  // A pick and a name are about one project's worktrees.
  const [pickedIn, setPickedIn] = useState(projectId)
  if (pickedIn !== projectId) {
    setPickedIn(projectId)
    setSelected(null)
    setName("")
  }

  const project = spine?.projects.find((p) => p.id === projectId)
  // The name it had, kept once the project is gone and the dialog is closing.
  const [projectName, setProjectName] = useState(project?.name ?? "project")
  if (project && project.name !== projectName) setProjectName(project.name)
  const adoptable = attachWorktreeEntries.filter((e) => e.adoptable)
  const attached = attachWorktreeEntries.filter(
    (e) => !e.adoptable && !e.being_removed && e.agent_id !== null,
  )
  // A worktree no agent of its own holds, but something is using: an agent
  // being created in it, a standalone agent living inside it, or the CLI of an
  // agent deleted a moment ago (worktree kept) still stopping. Telling the
  // user to delete the agent holding it would point at nobody, so it gets its
  // own group, and the server's reason sits on the row itself.
  const inUse = attachWorktreeEntries.filter(
    (e) => !e.adoptable && !e.being_removed && e.agent_id === null,
  )
  // A worktree whose agent was just deleted: its removal is already under
  // way, so it is neither free nor held, and it cannot be removed twice.
  const beingRemoved = attachWorktreeEntries.filter((e) => e.being_removed)

  // Resolve the holding agent's display name from the spine, the one place the
  // web derives an agent's name (`title || branch_name`).
  function agentName(entry: ProjectWorktreeEntryView): string | undefined {
    const session = spine?.sessions.find((s) => s.id === entry.agent_id)
    if (!session) return undefined
    return sessionLabel(session)
  }

  // The display name mirrors the TUI prompt: empty is rejected (a worktree
  // adoption requires a name) and a non-empty name must pass the agent-name
  // rules. The branch already exists, so this is a display name only.
  const empty = name.trim() === ""
  const invalidNonEmpty = !empty && !isValidAgentName(name)
  const disabled = !selected || empty || invalidNonEmpty
  const refusal = !selected
    ? "Pick an unused worktree first."
    : empty
      ? "Type a display name for the agent first."
      : invalidNonEmpty
        ? INVALID_AGENT_NAME
        : null

  function handleSelect(entry: ProjectWorktreeEntryView) {
    setSelected(entry.worktree_path)
    // Default the display name to the worktree's tail, like the TUI seeds the
    // managed worktree's relative name; the user can edit it.
    if (name.trim() === "") setName(sanitizeAgentName(pathTail(entry.worktree_path)))
  }

  function handleAttach() {
    if (disabled || !selected) return
    attachWorktree(projectId, selected, name.trim())
    closeAttachWorktree()
  }

  // Back goes UP to the project list; it is offered only when the user came down
  // from there. Cancel still closes the whole flow.
  function handleBack() {
    closeAttachWorktree()
    openNewAgentPicker("from_worktree")
  }

  return (
    <Modal
      kind="form"
      open={open}
      // Managing the worktrees of a deleted project is moot.
      vanished={open && project === undefined}
      onClose={closeAttachWorktree}
      onClosed={onClosed}
      size="xl"
      title={
        <>
          Worktrees in <InlineCode>{projectName}</InlineCode>
        </>
      }
      description="Every worktree dux manages for this project. Pick an unused one to start an agent on its existing branch, or remove one you are done with."
      // Up to the project list, offered only when the user came down from
      // there. Cancel still closes the whole flow.
      back={attachWorktreeFromPicker ? { label: "Projects", onSelect: handleBack } : undefined}
      submit={{
        label: "Create agent",
        tone: "primary",
        onSelect: handleAttach,
        disabledReason: refusal,
      }}
    >
      <ScrollArea className="h-64 shrink-0 rounded-md border max-md:h-auto max-md:min-h-40 max-md:flex-1">
        <ScrollList scroll="enclosing">
          {attachWorktreeLoading ? (
            <div className="flex h-64 items-center justify-center max-md:h-40">
              <Spinner kind="glyph" size="lg" tone="muted" />
            </div>
          ) : adoptable.length === 0 &&
            attached.length === 0 &&
            inUse.length === 0 &&
            beingRemoved.length === 0 &&
            attachWorktreeLeftovers.length === 0 ? (
            // The spinner's box, so the dialog keeps its height when the list
            // turns out empty.
            <div className="flex h-64 items-center justify-center max-md:h-40">
              <EmptyState
                size="compact"
                title="This project has no worktrees dux manages yet. Create an agent and one appears here."
              />
            </div>
          ) : (
            <ScrollListRows gap="none" semantics="none">
              {adoptable.map((entry) => {
                const isSelected = selected === entry.worktree_path
                return (
                  <div
                    key={entry.worktree_path}
                    className={`flex min-h-11 w-full items-center gap-1 pr-1 md:pointer-coarse:min-h-10 md:not-pointer-coarse:min-h-0 ${
                      isSelected ? "bg-accent" : "hover:bg-accent/60"
                    }`}
                  >
                    <SimpleTooltip content={rowTooltip(entry)}>
                      {/* min-h-11 gives a ≥44px touch target on phones; desktop
                         keeps the compact density via md:. */}
                      <button
                        type="button"
                        onClick={() => handleSelect(entry)}
                        className="flex min-h-11 min-w-0 flex-1 items-center gap-x-2.5 px-3 py-2 text-left max-md:flex-wrap md:pointer-coarse:min-h-10 md:not-pointer-coarse:min-h-0"
                      >
                        <WorktreeRowBody entry={entry} />
                      </button>
                    </SimpleTooltip>
                    <DropdownMenu>
                      <EllipsisMenuTrigger placement="row" aria-label="Worktree actions" />
                      <DropdownMenuContent align="end">
                        {/* Neutral colour: the trailing ellipsis and the
                           confirmation carry the danger, per the row-menu
                           convention. */}
                        <DropdownMenuItem
                          onClick={() => openDeleteWorktree(projectId, entry)}
                        >
                          <Trash2 />
                          Delete worktree…
                        </DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </div>
                )
              })}
              {attached.length > 0 ? (
                <>
                  <div className="pt-2">
                    <ModalGroupHeading inset caption="To remove one of these, delete the agent holding it.">
                      Already has an agent
                    </ModalGroupHeading>
                  </div>
                  {/* Deliberately no delete action: removing a worktree from under
                    * a live agent breaks the session, and deleting the agent is
                    * the path that already exists and already confirms. */}
                  {attached.map((entry) => (
                    <SimpleTooltip
                      key={entry.worktree_path}
                      content={rowTooltip(entry)}
                    >
                      <div className="flex min-h-11 cursor-not-allowed items-center gap-x-2.5 px-3 py-2 text-left opacity-70 max-md:flex-wrap md:min-h-0">
                        <WorktreeRowBody
                          entry={entry}
                          heldBy={agentName(entry)}
                          inUse={entry.in_use}
                        />
                      </div>
                    </SimpleTooltip>
                  ))}
                </>
              ) : null}
              {inUse.length > 0 ? (
                <>
                  <div className="pt-2">
                    <ModalGroupHeading inset caption="Something is still using these. Each row says what; remove one once that has finished.">
                      In use
                    </ModalGroupHeading>
                  </div>
                  {/* No actions: the server refuses a removal while the folder is
                    * in use, so offering one would only end in that refusal. */}
                  {inUse.map((entry) => (
                    <SimpleTooltip
                      key={entry.worktree_path}
                      content={rowTooltip(entry)}
                    >
                      <div className="flex min-h-11 cursor-not-allowed items-center gap-x-2.5 px-3 py-2 text-left opacity-70 max-md:flex-wrap md:min-h-0">
                        <WorktreeRowBody entry={entry} inUse={entry.reason} />
                      </div>
                    </SimpleTooltip>
                  ))}
                </>
              ) : null}
              {beingRemoved.length > 0 ? (
                <>
                  <div className="pt-2">
                    <ModalGroupHeading inset caption="Their agents were just deleted. dux removes each one once that agent has stopped.">
                      Being removed
                    </ModalGroupHeading>
                  </div>
                  {/* No actions: the removal is already running, and a second
                    * one would only race it. */}
                  {beingRemoved.map((entry) => (
                    <SimpleTooltip
                      key={entry.worktree_path}
                      content={rowTooltip(entry)}
                    >
                      <div className="flex min-h-11 cursor-not-allowed items-center gap-x-2.5 px-3 py-2 text-left opacity-70 max-md:flex-wrap md:min-h-0">
                        <WorktreeRowBody entry={entry} />
                      </div>
                    </SimpleTooltip>
                  ))}
                </>
              ) : null}
              {attachWorktreeLeftovers.length > 0 ? (
                <>
                  <div className="pt-2">
                    <ModalGroupHeading inset caption="git let go of these folders, but something was still writing into them when dux deleted them.">
                      Left over after a delete
                    </ModalGroupHeading>
                  </div>
                  {attachWorktreeLeftovers.map((folder) => (
                    <LeftoverRow
                      key={folder.path}
                      projectId={projectId}
                      folder={folder}
                    />
                  ))}
                </>
              ) : null}
            </ScrollListRows>
          )}
        </ScrollList>
      </ScrollArea>

      {selected ? (
        <div className="grid shrink-0 gap-1">
          <Input
            value={name}
            onChange={(e) => {
              const el = e.target
              const raw = el.value
              const caret = el.selectionStart ?? raw.length
              setName(sanitizeAgentName(raw))
              // Restore the caret after live sanitization shrinks the string,
              // so mid-string edits don't jump to the end (same as CreateAgent).
              const sanitized = sanitizeAgentName(raw)
              if (sanitized !== raw) {
                const next = Math.max(0, caret - (raw.length - sanitized.length))
                requestAnimationFrame(() => el.setSelectionRange(next, next))
              }
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault()
                handleAttach()
              }
            }}
            placeholder="Display name"
            aria-invalid={invalidNonEmpty}
            autoFocus
          />
          <span className="text-xs text-muted-foreground">
            Display name only: the branch already exists. Any name git
            accepts for a branch.
          </span>
        </div>
      ) : null}

    </Modal>
  )
}

// The per-project worktree manager: it lists every worktree dux manages for a
// project, adopts an unused one as an agent, and removes one no longer wanted.
// Its store surface is named `attachWorktree` rather than after these labels.
export function WorktreesDialog() {
  const open = useDux().attachWorktreeTarget !== null
  const life = useModalLifetime(open)
  return (
    <>
      {life.mounted ? (
        <WorktreesBody key={life.epoch} open={open} onClosed={life.onClosed} />
      ) : null}
      {/* Siblings, not children: the manager stays open behind a
         confirmation, so cancelling lands the user back on the list. */}
      <ConfirmDeleteWorktree />
      <ConfirmDeleteLeftoverFolder />
    </>
  )
}
