import { isValidAgentName } from "@/lib/agentName"
import { sessionLabel } from "@/lib/agentWorkspace"
import { chip, type Prose } from "@/lib/prose"
import type { CreateAgentTarget, DuxState } from "@/lib/store"

export type CreateAgentDialogKind = CreateAgentTarget["kind"] | "closed"

export interface CreateAgentDialogView {
  open: boolean
  kind: CreateAgentDialogKind
  // The project or agent it names is a chip; a generic fallback noun is prose.
  title: Prose
  description: string
  namePlaceholder: string
  nameAutoFocus: boolean
  showPrFields: boolean
  prPlaceholder: string
  showProjectPicker: boolean
  showCopyChanges: boolean
  submitLabel: string
}

export interface CreateAgentFormView {
  invalidName: boolean
  submitDisabled: boolean
}

export function createAgentDialogView(
  target: CreateAgentTarget | null,
  spine: DuxState["spine"],
): CreateAgentDialogView {
  if (target === null) return closedDialogView()
  if (target.kind === "fork") return forkDialogView(target, spine)
  if (target.kind === "pr") return prDialogView(target, spine)
  return newDialogView(target, spine)
}

function closedDialogView(): CreateAgentDialogView {
  return {
    open: false,
    kind: "closed",
    title: ["New agent"],
    description: "",
    namePlaceholder: "Branch name (optional)",
    nameAutoFocus: true,
    showPrFields: false,
    prPlaceholder: "",
    showProjectPicker: false,
    showCopyChanges: false,
    submitLabel: "Create agent",
  }
}

function newDialogView(
  target: Extract<CreateAgentTarget, { kind: "new" }>,
  spine: DuxState["spine"],
): CreateAgentDialogView {
  return {
    open: true,
    kind: "new",
    title: ["New agent in ", ...projectNamed(spine, target.projectId)],
    description:
      "Creates a git worktree + branch and launches the agent. Tick “Use randomized pet name” to autofill a generated name.",
    namePlaceholder: "Branch name (optional)",
    nameAutoFocus: true,
    showPrFields: false,
    prPlaceholder: "",
    showProjectPicker: false,
    showCopyChanges: true,
    submitLabel: "Create agent",
  }
}

function forkDialogView(
  target: Extract<CreateAgentTarget, { kind: "fork" }>,
  spine: DuxState["spine"],
): CreateAgentDialogView {
  const session = spine?.sessions.find((item) => item.id === target.sessionId)
  return {
    open: true,
    kind: "fork",
    title: ["Fork ", session ? chip(sessionLabel(session)) : "agent"],
    description:
      "Forks the agent into a new git worktree + branch (copying its uncommitted and untracked files) and launches a fresh session.",
    namePlaceholder: "Branch name",
    nameAutoFocus: true,
    showPrFields: false,
    prPlaceholder: "",
    showProjectPicker: false,
    showCopyChanges: false,
    submitLabel: "Fork agent",
  }
}

function prDialogView(
  target: Extract<CreateAgentTarget, { kind: "pr" }>,
  spine: DuxState["spine"],
): CreateAgentDialogView {
  if (target.projectId === null) return referenceFirstPrDialogView()
  return {
    open: true,
    kind: "pr",
    title: [
      "New agent from PR in ",
      ...projectNamed(spine, target.projectId),
    ],
    description:
      "Fetches the PR's head branch into a new git worktree and launches the agent. Paste a PR URL or enter a PR number. Leave the name blank to use the PR's branch name.",
    namePlaceholder: "Branch name (optional)",
    nameAutoFocus: false,
    showPrFields: true,
    prPlaceholder: "PR URL, #123, or 123",
    showProjectPicker: false,
    showCopyChanges: false,
    submitLabel: "Create from PR",
  }
}

function referenceFirstPrDialogView(): CreateAgentDialogView {
  return {
    open: true,
    kind: "pr",
    title: ["New agent from PR"],
    description:
      "Paste a pull request link, or type owner/repo#123. dux finds the project that repository is open in, fetches the PR's head branch into a new git worktree and launches the agent. Leave the name blank to use the PR's branch name.",
    namePlaceholder: "Branch name (optional)",
    nameAutoFocus: false,
    showPrFields: true,
    prPlaceholder: "Pull request link, or owner/repo#123",
    showProjectPicker: true,
    showCopyChanges: false,
    submitLabel: "Create from PR",
  }
}

function projectNamed(spine: DuxState["spine"], projectId: string): Prose {
  const name = spine?.projects.find((project) => project.id === projectId)?.name
  return name !== undefined ? [chip(name)] : ["project"]
}

export function createAgentFormView(
  kind: CreateAgentDialogKind,
  draft: string,
  prInput: string,
  resolvingPr: boolean,
  awaitingReviewCopyAgreement = false,
): CreateAgentFormView {
  const emptyName = draft.trim() === ""
  const invalidName = !emptyName && !isValidAgentName(draft)
  const emptyPr = prInput.trim() === ""
  return {
    invalidName,
    submitDisabled:
      invalidName ||
      (kind === "fork" && emptyName) ||
      (kind === "pr" && emptyPr) ||
      resolvingPr ||
      awaitingReviewCopyAgreement,
  }
}
