// Recreating a working copy an agent deleted from under itself: the pure half.
// What the confirmation says, and whether the action exists at all.
//
// The wording mirrors `dux_core::working_copy::recreate_confirm_body` word for
// word, and each side has a test asserting the exact sentences, so a change on
// one fails on the side that changed.

import {
  type AgentWorkspaceWire,
  type RecreateWire,
  type RunningTabs,
  matchWorkspace,
} from "./agentWorkspace"
import { chip, type Prose, proseText, quotedChip } from "./prose"

/** Whether the recreate action exists for this agent. Its own helper rather
 * than an inline check, because the menu, the dialog and the test all have to
 * agree: a managed working copy that is gone, and nothing else. dux never
 * creates, moves or removes a standalone agent's folder. */
export function canRecreateWorkingCopy(workspace: AgentWorkspaceWire): boolean {
  return matchWorkspace(workspace, {
    managed: (w) => w.worktree_missing === true,
    folder: () => false,
  })
}

/** The CLIs whose running tab follows a recreated working copy by itself,
 * mirroring `FOLDER_FOLLOWING_CLIS` in dux-core's `working_copy`. */
const FOLDER_FOLLOWING_CLIS = ["claude", "grok"]

/** What the confirmation is written from: the server's sorted running tabs and
 * restart offer. Which CLI follows the folder and which can resume is the
 * server's answer, so the browser never decides it. A server that predates the
 * field sends none, and the dialog then speaks about the agent's own provider
 * and offers no restart. */
export function recreateView(
  workspace: Extract<AgentWorkspaceWire, { kind: "managed" }>,
  provider: string,
): RecreateWire {
  if (workspace.recreate) return workspace.recreate
  const running: RunningTabs = { follows: [], restartable: [], left_running: [] }
  if (FOLDER_FOLLOWING_CLIS.includes(provider.toLowerCase()))
    running.follows.push(provider)
  else if (workspace.conversation_resumes === true)
    running.restartable.push(provider)
  else running.left_running.push(provider)
  return { running, restart_offer: [] }
}

/** The provider as a sentence names it: its own word, first letter capital. */
function providerName(provider: string): string {
  return provider.charAt(0).toUpperCase() + provider.slice(1)
}

/** Each provider once, in the order first met, as a sentence names it. */
function distinctNames(providers: string[]): string[] {
  const names: string[] = []
  for (const provider of providers) {
    const name = providerName(provider)
    if (!names.includes(name)) names.push(name)
  }
  return names
}

/** Several provider names as one subject, each marked as a name, with `last`
 * before the final one: "Codex", "Codex or Opencode". */
function providersJoined(names: string[], last: string): Prose {
  return names.flatMap((name, index): Prose => {
    if (index === 0) return [chip(name)]
    return [index === names.length - 1 ? last : ", ", chip(name)]
  })
}

/** Sentences joined with one space, skipping the empty ones. */
function sentences(parts: Prose[]): Prose {
  return parts
    .filter((part) => part.length > 0)
    .flatMap((part, index) => (index === 0 ? part : [" ", ...part]))
}

function leftRunningProse(providers: string[]): Prose {
  return [
    "A running ",
    ...providersJoined(distinctNames(providers), " or "),
    " tab is left running, because restarting it would lose its " +
      "conversation; if it cannot follow the folder, stop it and start it " +
      "again yourself.",
  ]
}

/** What the tabs still running in the deleted directory do once the working
 * copy is back, mirroring `dux_core::working_copy::recreate_running_tab_clause`:
 * one sentence per fate present, each naming every provider it is about.
 *
 * Measured against the real CLIs. The Claude CLI never notices its directory
 * going, and once one exists at the same path again it keeps working in it with
 * its conversation intact; Grok behaves like it, assumed rather than measured
 * separately. The Codex CLI answers every turn with "invalid cwd"
 * from the moment the directory goes and stays stuck there after the recreate,
 * until it is quit and resumed in the new folder. OpenCode and Copilot have not
 * been measured, so they get the cautious answer rather than a promise. */
export function recreateRunningTabClause(running: RunningTabs): string {
  return proseText(recreateRunningTabProse(running))
}

/** The same clause with each provider marked, for the web to chip. */
function recreateRunningTabProse(running: RunningTabs): Prose {
  const parts: Prose[] = []
  if (running.follows.length > 0)
    parts.push([
      "A running ",
      ...providersJoined(distinctNames(running.follows), " or "),
      " tab keeps working in the recreated copy by itself.",
    ])
  if (running.restartable.length > 0)
    parts.push([
      "A running ",
      ...providersJoined(distinctNames(running.restartable), " or "),
      " tab cannot follow the folder by itself; restarting it continues its " +
        "conversation in the recreated copy.",
    ])
  if (running.left_running.length > 0)
    parts.push(leftRunningProse(running.left_running))
  return sentences(parts)
}

/** The label of the confirmation's restart checkbox, mirroring
 * `dux_core::working_copy::recreate_restart_checkbox_prose`. One entry per tab,
 * so two tabs of one CLI read as plural while the CLI is named once. */
export function recreateRestartCheckboxProse(restartable: string[]): Prose {
  return [
    "Restart the ",
    ...providersJoined(distinctNames(restartable), " and "),
    restartable.length === 1
      ? " tab so it continues in the new copy"
      : " tabs so they continue in the new copy",
  ]
}

/** The body of the recreate confirmation.
 *
 * All three branch arms are stated because asking the server which one applies
 * would cost a round trip to open a dialog; the status that follows names the
 * arm that actually ran. The path is the server's, home-collapsed there,
 * because the browser is not necessarily on the server's machine.
 *
 * `conversationResumes` is the agent's provider's own answer rather than a
 * constant: a provider that keeps no directory-scoped history (copilot ships
 * with no resume arguments) would have this dialog promising something that
 * cannot happen, and the same path buys it nothing.
 *
 * `running` are the tabs running right now, or the agent's own when none is,
 * from `recreateView`.
 *
 * The path and every branch are marked for the web to chip. dux-core's
 * `recreate_confirm_prose` builds the same segments for the terminal UI, and
 * both are pinned by `crates/dux-core/tests/fixtures/prose_cross_language.json`. */
export function recreateConfirmProse(
  worktreeLabel: string,
  branchName: string,
  sourceBranch: string,
  conversationResumes: boolean,
  running: RunningTabs,
): Prose {
  const conversation = conversationResumes
    ? `The conversation may resume, because the agent's CLI keys its history ` +
      `by directory path and dux recreates the working copy at the same path.`
    : `The conversation will not resume: this agent's CLI has no way to pick ` +
      `a conversation back up, so it starts fresh wherever it runs.`
  return [
    "Recreate the working copy for this agent at ",
    chip(worktreeLabel),
    "?\n\nIf branch ",
    quotedChip(branchName),
    ` still exists locally, dux checks it out there ` +
      `again. If it is gone locally but still on the remote, dux creates it ` +
      `again from `,
    quotedChip(`origin/${branchName}`),
    `, holding everything that had been ` +
      `pushed. If it is gone everywhere, dux creates it again from `,
    quotedChip(sourceBranch),
    `, and the commits that branch held are not coming ` +
      `back.\n\n` +
      `Any code changes that were in the old directory are gone either way: this ` +
      `puts the directory back, not its contents. ${conversation}\n\n`,
    ...recreateRunningTabProse(running),
    ` A terminal still open in the old directory keeps ` +
      `working in a directory that is gone; close it and open one in the ` +
      `recreated copy.`,
  ]
}
