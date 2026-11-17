import { useEffect, useRef, useState } from "react"

import { AgentNameInput } from "@/components/AgentNameFields"
import { FolderBrowseList } from "@/components/FolderBrowseList"
import { ProjectShortcutList } from "@/components/ProjectShortcutList"
import { usePrefersReducedMotion } from "@/hooks/use-reduced-motion"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { InlineCode } from "@/components/ui/inline-code"
import { isValidAgentName, sanitizeAgentName } from "@/lib/agentName"
import { standaloneAgentDefaultName } from "@/lib/agentWorkspace"
import type { ProjectShortcutView } from "@/lib/types"
import {
  browseDir,
  closeStandaloneAgentPicker,
  createStandaloneAgent,
  useDux,
} from "@/lib/store"

/**
 * Pick a folder you already have and run an agent in it.
 *
 * The browsing is the add-project picker's, through the shared `FolderBrowseList`
 * and the store's one browse slice. That picker's inspection ladder is
 * deliberately absent: a standalone agent accepts whatever is there, initializes
 * nothing and never modifies the folder, so there is nothing to check.
 *
 * Above the explorer sit the user's projects as shortcuts. Picking one is
 * browsing to that project's folder and choosing it, nothing more: the agent
 * is an ordinary standalone agent with no tie to the project. The list is the
 * server's, rendered as it arrives, and is absent when there are no projects.
 *
 * The popup is a column of two parts: a body that scrolls, and the footer
 * below it, outside the scroller, so Cancel and Create agent are on screen at
 * every height whatever the body holds. Inside the body the explorer is the
 * one part that gives up height; only once it is at its floor does the body
 * scroll.
 *
 * A project pick targets its folder only once the explorer has really landed
 * there: a browse that fails, or that a newer browse replaced, leaves nothing
 * targeted, so Create agent can never act on a folder the explorer never
 * reached.
 *
 * Every refusal is the server's and arrives as a toast, shared with the terminal
 * UI so the two surfaces cannot answer the same question differently.
 */
function StandaloneAgentBrowser() {
  const { browsePath, browseEntries, browseLoading, standaloneProjectShortcuts } =
    useDux()
  const [selected, setSelected] = useState<string | null>(null)
  // The project whose shortcut put the explorer where it is, so its row paints
  // as chosen. Tracked by id rather than by comparing paths: the explorer's
  // path is whatever spelling the server echoed.
  const [pickedProject, setPickedProject] = useState<string | null>(null)
  const [name, setName] = useState("")
  // Choosing a folder reveals the name field at the end of the scrolling
  // body, which on a short screen is below the fold: bring it into view, so
  // the choice visibly did something. Scrolling only; focus stays where it
  // was (the field has never taken it by itself, and on a phone that would
  // raise the keyboard). No glide under reduced motion.
  const reducedMotion = usePrefersReducedMotion()
  const nameBlock = useRef<HTMLDivElement | null>(null)
  // Asked again when the explorer finishes listing the chosen folder (its
  // rows land a moment after the choice and can push the field back down),
  // and whenever the block itself changes size: the body gaining a scrollbar
  // narrows it, a long path then wraps onto another line, and the block grows
  // after the first reveal was aimed.
  useEffect(() => {
    const block = nameBlock.current
    if (selected === null || block === null) return
    const reveal = (behavior: ScrollBehavior) =>
      block.scrollIntoView?.({ block: "nearest", behavior })
    reveal(reducedMotion ? "auto" : "smooth")
    if (typeof ResizeObserver !== "function") return
    // An observer reports once as it starts; that report is not a change,
    // and answering it would cut the glide short.
    let started = false
    const resized = new ResizeObserver(() => {
      if (started) reveal("auto")
      started = true
    })
    resized.observe(block)
    return () => resized.disconnect()
  }, [selected, browseLoading, browseEntries, reducedMotion])
  const invalidName = name !== "" && !isValidAgentName(name)

  function handleCreate() {
    if (!selected || invalidName) return
    createStandaloneAgent(selected, name)
    setSelected(null)
    setPickedProject(null)
    setName("")
    closeStandaloneAgentPicker()
  }

  function handlePick(row: ProjectShortcutView) {
    // Nothing is targeted while the explorer is on its way.
    setSelected(null)
    setPickedProject(null)
    void browseDir(row.path).then((landed) => {
      // Null: the browse failed (the store said why) or was replaced.
      if (landed === null) return
      // Exactly what browsing there and pressing the pinned row does.
      setSelected(landed)
      setPickedProject(row.project_id)
    })
  }

  return (
    <DialogContent
      // The popup itself never scrolls: its body does, and the footer sits
      // below the body, so it cannot leave the screen.
      className="flex flex-col overflow-hidden sm:max-w-xl"
      showCloseButton={false}
    >
      {/* The scrolling body. It reaches out over the popup's padding and pads
          itself back in, so a focus ring on its edge is not clipped. */}
      <div
        data-standalone-picker-body
        className="-mx-4 -mt-4 flex min-h-0 flex-col gap-4 overflow-y-auto px-4 pt-4"
      >
        <DialogHeader className="shrink-0">
          <DialogTitle>New standalone agent</DialogTitle>
          {/* The words "standalone agent" appear here on purpose: every other
            * surface uses that name, so the dialog that creates one teaches it. */}
          <span className="text-xs text-muted-foreground">
            Pick any folder and a standalone agent runs there directly: no branch,
            no worktree, and dux never creates, moves or removes the folder.
          </span>
        </DialogHeader>

        {standaloneProjectShortcuts &&
        standaloneProjectShortcuts.rows.length > 0 ? (
          <ProjectShortcutList
            list={standaloneProjectShortcuts}
            pickedProjectId={selected === null ? null : pickedProject}
            onPick={handlePick}
          />
        ) : null}

        <FolderBrowseList
          shrinkable
          path={browsePath}
          entries={browseEntries}
          loading={browseLoading}
          commitLabel="Run an agent here"
          committed={selected === browsePath}
          onCommit={(path) => {
            // Another folder chosen by hand is no longer the project's pick.
            if (path !== selected) setPickedProject(null)
            setSelected(path)
          }}
          onOpen={(entry) => {
            // Navigating away abandons the pending choice, so the footer can
            // never act on a folder the user has left.
            setSelected(null)
            setPickedProject(null)
            void browseDir(entry.path)
          }}
        />

        {selected ? (
          <div
            ref={nameBlock}
            data-standalone-name
            className="flex shrink-0 flex-col gap-2 pb-px"
          >
            {/* Empty is the ordinary case: the server names the agent after the
              * folder. No branch is created, but the name follows the same rules
              * as every other agent's, so the field is the New agent dialog's.
              * The placeholder is derived through the twin of the server's
              * folder conversion, so it cannot promise another name. */}
            <AgentNameInput
              value={name}
              onChange={(raw) => setName(sanitizeAgentName(raw))}
              onSubmit={handleCreate}
              placeholder={`Agent name (optional, defaults to "${standaloneAgentDefaultName(selected)}")`}
              invalid={invalidName}
              generating={false}
              autoFocus={false}
            />
            <p className="text-xs text-muted-foreground">
              Any name git accepts for a branch.
            </p>
            <span className="text-xs text-muted-foreground">
              <InlineCode>{selected}</InlineCode>
            </span>
          </div>
        ) : null}
      </div>

      <DialogFooter className="shrink-0">
        <Button variant="outline" onClick={closeStandaloneAgentPicker}>
          Cancel
        </Button>
        <Button onClick={handleCreate} disabled={!selected || invalidName}>
          Create agent
        </Button>
      </DialogFooter>
    </DialogContent>
  )
}

export function StandaloneAgentDialog() {
  const { standaloneAgentPickerOpen } = useDux()
  return (
    <Dialog
      open={standaloneAgentPickerOpen}
      onOpenChange={(open) => {
        if (!open) closeStandaloneAgentPicker()
      }}
    >
      {/* Mounted only while open, so the browse slice is loaded fresh on every
          open rather than showing wherever the last pick left off. */}
      {standaloneAgentPickerOpen && <StandaloneAgentBrowser />}
    </Dialog>
  )
}
