import { type ReactNode, useEffect, useRef, useState } from "react"
import { FolderOpen } from "lucide-react"

import { AgentNameInput } from "@/components/AgentNameFields"
import { FolderBrowseList } from "@/components/FolderBrowseList"
import { Modal, type ModalButtonAction, useModalLifetime } from "@/components/modal"
import { ProjectShortcutList } from "@/components/ProjectShortcutList"
import { Button } from "@/components/ui/button"
import { usePrefersReducedMotion } from "@/hooks/use-reduced-motion"
import { FormGroup } from "@/components/ui/form-group"
import { InlineCode } from "@/components/ui/inline-code"
import {
  agentFolderName,
  INVALID_AGENT_NAME,
  isValidAgentName,
  sanitizeAgentName,
} from "@/lib/agentName"
import { standaloneAgentDefaultName } from "@/lib/agentWorkspace"
import { browseApi } from "@/lib/browseApi"
import { type Prose, prose, quotedChip, renderProse } from "@/lib/prose"
import { sessionsApi } from "@/lib/sessionsApi"
import type { ProjectShortcutView } from "@/lib/types"
import {
  browseDir,
  closeStandaloneAgentPicker,
  createStandaloneAgent,
  createStandaloneAgentInNewFolder,
  useDux,
} from "@/lib/store"

/** Why the server's create would refuse a name for its folder: something is
 * already there, or an agent is recorded there (`agent`). */
interface FolderTaken {
  agent: string | null
}

/** Why the name's folder is taken, as the create says it. */
function folderTakenWords(taken: FolderTaken): Prose {
  return taken.agent === null
    ? prose`A folder with that name is already there.`
    : prose`Agent ${quotedChip(taken.agent)} already has that folder.`
}

/** Why Create agent waits on the name step, or `null` when it may go. */
function newFolderBlocker(name: string, taken: FolderTaken | null): string | Prose | null {
  if (name === "") return "Give the agent a name first."
  if (!isValidAgentName(name)) return INVALID_AGENT_NAME
  if (taken !== null) return folderTakenWords(taken)
  return null
}

/** What one step puts in the modal: its words, its submit and its body. */
interface StepParts {
  title: string
  description: string
  submit: ModalButtonAction
  body: ReactNode
}

/** The name step: dux creates the agent's folder, named after it. A taken name
 * is refused here, and again atomically by the server's create. The box starts
 * with a generated name only while random names are on by default, as every
 * new-agent name does. */
function useNewFolderStep(onUseExisting: () => void): StepParts {
  const randomizeByDefault = useDux().bootstrap?.randomize_agent_names_by_default ?? false
  // Read once, at opening, as the New agent dialog reads it.
  const [randomize] = useState(randomizeByDefault)
  const [name, setName] = useState("")
  const [generating, setGenerating] = useState(randomize)
  const [rootLabel, setRootLabel] = useState<string | null>(null)
  // The latest answer about whether the create would refuse a name for its
  // folder, kept with the name it answers so a stale one never speaks for
  // another.
  const [takenAnswer, setTakenAnswer] = useState<{
    name: string
    taken: FolderTaken | null
  } | null>(null)

  useEffect(() => {
    let live = true
    if (randomize) {
      browseApi
        .agentName()
        .then((reply) => {
          if (live) setName(sanitizeAgentName(reply.name))
        })
        .catch(() => {})
        .finally(() => {
          if (live) setGenerating(false)
        })
    }
    sessionsApi
      .newAgentFolder("")
      .then((reply) => {
        if (live) setRootLabel(reply.root_label)
      })
      .catch(() => {})
    return () => {
      live = false
    }
  }, [randomize])

  const valid = name !== "" && isValidAgentName(name)
  useEffect(() => {
    if (!valid) return
    let live = true
    // Asked once typing settles, not on every key.
    const timer = setTimeout(() => {
      sessionsApi
        .newAgentFolder(name)
        .then((reply) => {
          if (!live) return
          setRootLabel(reply.root_label)
          setTakenAnswer({
            name,
            taken: reply.taken ? { agent: reply.taken_by_agent ?? null } : null,
          })
        })
        .catch(() => {})
    }, 150)
    return () => {
      live = false
      clearTimeout(timer)
    }
  }, [name, valid])

  const taken = takenAnswer !== null && takenAnswer.name === name ? takenAnswer.taken : null
  const blocker = newFolderBlocker(name, taken)
  const error =
    name !== "" && !valid
      ? INVALID_AGENT_NAME
      : taken !== null
        ? renderProse(prose`${folderTakenWords(taken)} Pick another name.`)
        : null

  function handleCreate() {
    if (blocker !== null || generating) return
    createStandaloneAgentInNewFolder(name)
    closeStandaloneAgentPicker()
  }

  return {
    title: "New standalone agent",
    description:
      "Give the agent a name and dux creates a folder for it to run in: no project, no branch, no worktree.",
    submit: {
      label: "Create agent",
      tone: "primary",
      onSelect: handleCreate,
      disabledReason: generating ? "dux is still picking a name for the agent." : blocker,
    },
    body: (
      <>
        <FormGroup
          layout="stacked"
          control="single"
          label="Agent name"
          help={
            <>
              The agent runs in{" "}
              <InlineCode>{newAgentFolderLabel(rootLabel, valid ? name : null)}</InlineCode>. dux
              creates the folder.
            </>
          }
          error={error}
        >
          {(field) => (
            <AgentNameInput
              field={field}
              value={name}
              onChange={(raw) => setName(sanitizeAgentName(raw))}
              onSubmit={handleCreate}
              generating={generating}
              autoFocus={false}
            />
          )}
        </FormGroup>
        {/* 40px under a finger and on a phone; on a computer its neighbours, the
          * field and the footer's rule, are clear of it by the body's gap. */}
        <Button
          variant="ghost"
          className="self-start max-md:min-h-10 pointer-coarse:min-h-10"
          onClick={onUseExisting}
        >
          <FolderOpen data-icon="inline-start" />
          Use an existing folder or project…
        </Button>
      </>
    ),
  }
}

/** The folder a new agent called `name` gets, home-collapsed as the server
 * says it, with `…` for a part not known yet. */
function newAgentFolderLabel(rootLabel: string | null, name: string | null): string {
  return `${rootLabel ?? "…"}/${name === null ? "…" : agentFolderName(name)}`
}

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
 * Inside the modal's scrolling body the explorer is the one part that gives
 * up height; only once it is at its floor does the body scroll.
 *
 * A project pick targets its folder only once the explorer has really landed
 * there: a browse that fails, or that a newer browse replaced, leaves nothing
 * targeted, so Create agent can never act on a folder the explorer never
 * reached.
 *
 * Every refusal is the server's and arrives as a toast, shared with the terminal
 * UI so the two surfaces cannot answer the same question differently.
 */
function useExistingFolderStep(): StepParts {
  const { browsePath, browseEntries, browseLoading, standaloneProjectShortcuts, spine } =
    useDux()
  // One shortcut row per project, so the projects the app already holds say
  // how much room the list will take before the shortcuts answer.
  const knownProjects = spine?.projects.length ?? null
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
  // Asked again when the project list arrives above the explorer after a
  // folder was already chosen (it pushes everything below it down), and
  // whenever the block itself changes size: the body gaining a scrollbar
  // narrows it, a long path then wraps onto another line, and the block grows
  // after the first reveal was aimed.
  const shortcutsShown = (standaloneProjectShortcuts?.rows.length ?? 0) > 0
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
  }, [selected, shortcutsShown, reducedMotion])
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

  return {
    title: "Use an existing folder or project",
    // The words "standalone agent" appear here on purpose: every other
    // surface uses that name, so the dialog that creates one teaches it.
    description:
      "Pick any folder and a standalone agent runs there directly: no branch, no worktree, and dux never creates, moves or removes the folder.",
    submit: {
      label: "Create agent",
      tone: "primary",
      onSelect: handleCreate,
      disabledReason: !selected
        ? "Choose a folder for the agent to run in first."
        : invalidName
          ? INVALID_AGENT_NAME
          : null,
    },
    body: (
      <>
      {standaloneProjectShortcuts === null ? (
        knownProjects === 0 ? null : (
          <ProjectShortcutList
            list={null}
            expectedRows={knownProjects}
            pickedProjectId={null}
            onPick={handlePick}
          />
        )
      ) : shortcutsShown ? (
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
            * The default is derived through the twin of the server's folder
            * conversion, so it cannot promise another name, and it is said
            * under the field, where a long one is not cut off. */}
          <FormGroup
            layout="stacked"
            control="single"
            spokenName="Agent name"
            help={
              invalidName ? null : (
                <>
                  Left empty, the agent is called{" "}
                  <InlineCode>{standaloneAgentDefaultName(selected)}</InlineCode>. Any name git
                  accepts for a branch works.
                </>
              )
            }
            notes={[{ words: <>The agent runs in <InlineCode>{selected}</InlineCode>.</> }]}
            error={invalidName ? INVALID_AGENT_NAME : null}
          >
            {(field) => (
              <AgentNameInput
                field={field}
                value={name}
                onChange={(raw) => setName(sanitizeAgentName(raw))}
                onSubmit={handleCreate}
                placeholder="Agent name (optional)"
                generating={false}
                autoFocus={false}
              />
            )}
          </FormGroup>
        </div>
      ) : null}
      </>
    ),
  }
}

/** One modal in two steps, so its history entry carries across them; each step
 * keeps what it holds while the other is shown. */
function StandaloneAgentSteps({ open, onClosed }: { open: boolean; onClosed: () => void }) {
  const [step, setStep] = useState<"name" | "folder">("name")
  const named = useNewFolderStep(() => setStep("folder"))
  const existing = useExistingFolderStep()
  const shown = step === "name" ? named : existing
  return (
    <Modal
      kind="form"
      open={open}
      onClose={closeStandaloneAgentPicker}
      onClosed={onClosed}
      size={step === "name" ? "lg" : "xl"}
      step={step}
      // The picker's own height, rather than the shorter name step's it comes
      // from, so the explorer is not squeezed.
      height={step === "name" ? "content" : "h680"}
      back={
        step === "folder"
          ? { label: "New standalone agent", onSelect: () => setStep("name") }
          : undefined
      }
      title={shown.title}
      description={shown.description}
      submit={shown.submit}
    >
      {shown.body}
    </Modal>
  )
}

export function StandaloneAgentDialog() {
  const { standaloneAgentPickerOpen } = useDux()
  const life = useModalLifetime(standaloneAgentPickerOpen)
  // Keyed by the open, so the name and the choice start afresh on every one.
  return life.mounted ? (
    <StandaloneAgentSteps
      key={life.epoch}
      open={standaloneAgentPickerOpen}
      onClosed={life.onClosed}
    />
  ) : null
}
