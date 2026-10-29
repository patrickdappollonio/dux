// Scoped REST client for agent-session mutations. Requests include the
// connection id so server-side operation status is routed back to the
// initiating client; failures surface as `SessionsApiError`.

import { forceConnectedQuery } from "./attached"
import { createJsonRequest } from "./jsonRequest"
import type { BranchOrigin } from "./deleteAgent"
import type { Prose } from "./prose"
import type {
  AcceptedOperation,
  SessionView,
  StartupLogContent,
  StartupLogsList,
} from "./types"

// A failed sessions REST call. `status` is 0 for a transport failure with no
// response, and `body` carries the server's parsed JSON error when there was one,
// which is how the existing-branch 409 delivers its confirm payload.
export class SessionsApiError extends Error {
  readonly status: number
  readonly body: unknown

  constructor(message: string, status: number, body: unknown = null) {
    super(message)
    this.name = "SessionsApiError"
    this.status = status
    this.body = body
  }
}

// The discriminated create body the server matches on `kind`. Without
// `use_existing_branch`, a name matching an existing branch is refused with a
// confirmable 409 that the client re-sends with the flag set.
export type CreateSessionBody =
  | {
      kind: "new"
      project_id: string
      name?: string
      copy_uncommitted_changes?: boolean
      use_existing_branch?: boolean
    }
  | { kind: "fork"; session_id: string; name?: string }
  | { kind: "from_worktree"; project_id: string; worktree_path: string; name?: string }
  | {
      kind: "from_pr"
      project_id: string
      pr: string
      name?: string
      // The user's agreement to a review copy of the pull request, and the
      // identifier of the warning it answers (from the server's refusal).
      review_copy?: boolean
      review_copy_warned?: string
      ask_id?: string
    }
  // A STANDALONE agent: it carries no project (it belongs to none), and it is
  // the only kind that carries a provider, because the others take their
  // project's default and this one has no project to take one from.
  | {
      kind: "standalone"
      folder: string
      name?: string
      provider?: string
    }

// The parsed body of the server's existing-branch refusal (409). Present on a
// `SessionsApiError` when the server asks the client to confirm attaching a new
// agent to an existing branch's history rather than silently adopting it.
export interface ExistingBranchConflict {
  name: string
  location: "local" | "remote"
}

/** Extract the existing-branch conflict from a create error, or null. */
export function existingBranchConflict(e: unknown): ExistingBranchConflict | null {
  if (
    e instanceof SessionsApiError &&
    e.status === 409 &&
    e.body !== null &&
    typeof e.body === "object" &&
    "existing_branch" in e.body
  ) {
    const eb = (e.body as { existing_branch: unknown }).existing_branch
    if (
      eb !== null &&
      typeof eb === "object" &&
      "name" in eb &&
      "location" in eb &&
      typeof (eb as { name: unknown }).name === "string"
    ) {
      const loc = (eb as { location: unknown }).location
      return {
        name: (eb as { name: string }).name,
        location: loc === "remote" ? "remote" : "local",
      }
    }
  }
  return null
}

// PATCH body for a session. Every field is optional; an omitted field is left
// untouched. Setting `provider` triggers a pending reconnect server-side.
export interface PatchSessionBody {
  title?: string
  // Also rename the git branch to `title`; read only with a `title`.
  rename_branch?: boolean
  provider?: string
  auto_reopen?: boolean
}

// Parse a response body as JSON, returning null for an empty string or invalid
// JSON. Used to attach the server's structured error payload (e.g. the
// existing-branch 409) without letting a non-JSON body throw.
function parseJsonOrNull(text: string): unknown {
  if (!text) return null
  try {
    return JSON.parse(text)
  } catch {
    return null
  }
}

const request = createJsonRequest(
  (message, status, responseText) =>
    new SessionsApiError(message, status, parseJsonOrNull(responseText)),
)

// What the server made of a typed pull-request reference. Callers branch on
// `projects.length`: one proceeds, several ask, none reports and offers the picker.
// `repository` is `host/owner/repo`, or `owner/repo` when the reference named no
// host, which dux must not fill in as github.com.
export interface ResolvedPullRequestReference {
  repository: string | null
  number: number | null
  projects: { id: string; name: string }[]
  // How many projects the server could not inspect at all, and a clause naming them.
  // They are unknowns rather than non-matches, so an empty `projects` must not be
  // reported as "no project is a checkout of that repository" while any remain.
  uninspected_count: number
  uninspected_summary: string | null
  // Present on a zero-match answer when the repository could be cloned: the address
  // dux would clone from and the pull request's facts as `gh` just reported them.
  clone?: PullRequestCloneOffer
  // Present instead when an offer was possible but `gh` could not look the pull
  // request up: the lookup's own sentence.
  clone_unavailable?: string
}

export interface PullRequestCloneOffer {
  address: string
  host: string
  owner_repo: string
  number: number
  title: string
  state: string
  head_branch: string
  // The pull request's head commit, when `gh` reported it.
  head_oid?: string
  review_copy: ReviewCopyOutlook
}

// Whether the agent will need a review copy, told before anything is cloned or
// added. `reason` and `copy_branch` travel back with the user's agreement.
export type ReviewCopyOutlook =
  | { state: "not_needed" }
  | { state: "unknown" }
  | {
      state: "needed"
      reason: string
      branch: string
      copy_branch: string
      warning: Prose
    }
  | { state: "no_free_name"; reason: string; branch: string; message: Prose }

// What a folder holds for the pull request's repository. For a checkout,
// `review_copy` replaces the offer's: it is computed against that folder.
export interface PullRequestCloneDestination {
  state: "free" | "checkout"
  path: string
  review_copy?: ReviewCopyOutlook
}

// One branch the delete dialog offers, pinned to the commit a tick deletes, and
// who the server judged made it.
export interface OfferedBranch {
  name: string
  oid: string
  origin: BranchOrigin
}

type UnpushedPayload = { count: number; has_remote_refs: boolean }

// The answer to `GET /api/v1/sessions/:id/branch-unpushed`: the branch the
// worktree is on (the first box, `null` when git could not read it) and every
// other branch it has been on (the second box), each box with how much of its
// work exists on no remote, or why git could not count it.
export interface BranchOffer {
  // The branch the worktree was on when this was read.
  current_name: string
  current: OfferedBranch | null
  current_unpushed: UnpushedPayload | null
  current_unpushed_error: string | null
  others: OfferedBranch[]
  others_unpushed: UnpushedPayload | null
  others_unpushed_error: string | null
  // What ticking both boxes loses.
  both_unpushed: UnpushedPayload | null
  both_unpushed_error: string | null
  // Branches the agent was on that are the project's own: offered in no box,
  // and never deleted.
  protected: string[]
}

// One box's answer on a delete. A ticked box carries exactly the entries the
// offer pinned; an unticked one carries none. `unchecked` says why a box had
// no offer to answer from (still checking, or the check failed), and the
// server then keeps every branch.
export interface BranchBoxAnswer {
  delete: boolean
  branches: { name: string; oid: string }[]
  unchecked?: "pending" | "failed" | "not_offered"
  // Every branch the box showed, ticked or not.
  offered?: string[]
}

// Both boxes' answers. Both are always sent: a box left out means nobody was
// asked, and the server would then apply its own default.
export interface DeleteBranchAnswers {
  current: BranchBoxAnswer
  others: BranchBoxAnswer
}

export const sessionsApi = {
  // 201 with the created session once it surfaces, otherwise 202 with the
  // operation id to correlate the outcome on the events socket. Nothing reads
  // the deferred half today; the type says it exists so a reader cannot assume
  // a `SessionView` is always what came back.
  create: (body: CreateSessionBody) =>
    request<SessionView | AcceptedOperation>("POST", "/api/v1/sessions", body),
  // Ask which project a typed pull-request reference belongs to. A read: it
  // starts nothing, so a refusal (unreadable text, a bare number, a host dux
  // may not ask about) comes back as a 400 with the reason rather than a toast.
  resolvePullRequest: (reference: string) =>
    request<ResolvedPullRequestReference>(
      "POST",
      "/api/v1/pull-requests/resolve",
      { reference },
    ),
  // Say what a folder holds for the pull request's repository. A read: it
  // creates nothing, and a refusal is a 400 with the sentence to show.
  checkCloneDestination: (body: {
    path: string
    host: string
    owner_repo: string
    number: number
    head_branch: string
    head_oid?: string
  }) =>
    request<PullRequestCloneDestination>(
      "POST",
      "/api/v1/pull-requests/clone-destination",
      body,
    ),
  // `branches` is the delete dialog's two branch boxes, sent as the body. `null`
  // means no box was on screen (the worktree is kept, or a standalone agent),
  // and the request then carries no body at all.
  remove: (
    id: string,
    deleteWorktree: boolean,
    branches: DeleteBranchAnswers | null,
    accepted: readonly string[] | null = null,
  ) =>
    request<void>(
      "DELETE",
      `/api/v1/sessions/${encodeURIComponent(id)}?delete_worktree=${deleteWorktree}` +
        forceConnectedQuery(accepted, false),
      branches === null ? undefined : { branches },
    ),
  // What the delete dialog's branch boxes offer, each pinned to the commit a
  // tick deletes, and how much work each would destroy.
  branchUnpushed: (id: string) =>
    request<BranchOffer>(
      "GET",
      `/api/v1/sessions/${encodeURIComponent(id)}/branch-unpushed`,
    ),
  // Whether renaming the agent's branch would leave a pushed copy behind on the
  // old name. `null` when git could not answer; the dialog then warns about
  // nothing it does not know.
  branchPushed: (id: string) =>
    request<{ pushed: boolean | null }>(
      "GET",
      `/api/v1/sessions/${encodeURIComponent(id)}/branch-pushed`,
    ),
  patch: (id: string, body: PatchSessionBody) =>
    request<{ provider_change?: string }>(
      "PATCH",
      `/api/v1/sessions/${encodeURIComponent(id)}`,
      body,
    ),
  reconnect: (id: string, force: boolean, accepted: readonly string[] | null = null) =>
    request<void>(
      "POST",
      `/api/v1/sessions/${encodeURIComponent(id)}/reconnect` +
        forceConnectedQuery(accepted, true),
      { force },
    ),
  // Stop the agent's processes (it detaches; it is NOT deleted). `force` skips
  // the shutdown grace and ends them at once, for the Task Manager's "Stop
  // everything"; without it the agent is asked to shut down and given the
  // configured wait. A non-2xx throws.
  kill: (id: string, force = false, accepted: readonly string[] | null = null) =>
    request<void>(
      "POST",
      `/api/v1/sessions/${encodeURIComponent(id)}/kill` +
        forceConnectedQuery(accepted, true),
      { force },
    ),
  // Check the agent's branch out again at the path its working copy used to
  // occupy. Answers with a keyed busy on the status stream; a refusal is a 400
  // and throws. `restartTabs` also restarts the running tabs that cannot follow
  // the folder, which stops them, so it is guarded like a stop: somebody else
  // attached is a 409 unless `accepted` names who the dialog showed.
  recreateWorkingCopy: (
    id: string,
    restartTabs: boolean,
    accepted: readonly string[] | null = null,
  ) =>
    request<void>(
      "POST",
      `/api/v1/sessions/${encodeURIComponent(id)}/recreate-working-copy` +
        forceConnectedQuery(accepted, true),
      { restart_tabs: restartTabs },
    ),
  // Attaches a pull request from the raw typed reference. Replies 202 with the keyed
  // status op id, and the outcome rides the status stream; a synchronous refusal is a
  // 400 and throws.
  attachPullRequest: (id: string, pr: string) =>
    request<{ op_id: string }>(
      "PUT",
      `/api/v1/sessions/${encodeURIComponent(id)}/pull-request`,
      { pr },
    ),
  // Detach the agent's pull request: the pin goes if there is one, the badge
  // clears, and dux stops looking for a PR on this agent. Synchronous; the
  // info status rides the stream. Applies to an autodetected association too.
  detachPullRequest: (id: string) =>
    request<void>(
      "DELETE",
      `/api/v1/sessions/${encodeURIComponent(id)}/pull-request`,
    ),
  // The way back from a detach: switch autodetection on again and check once
  // now. Synchronous, shaped like the detach beside it.
  resumePullRequestAutodetection: (id: string) =>
    request<void>(
      "POST",
      `/api/v1/sessions/${encodeURIComponent(id)}/pull-request/autodetect`,
    ),
  // Flat model: reorder every agent as one global list. `sessionIds` must be the
  // complete session set, in the desired order (the server validates strictly).
  reorderGlobal: (sessionIds: string[]) =>
    request<void>("POST", "/api/v1/sessions/reorder-global", {
      session_ids: sessionIds,
    }),
  // Re-run the agent's project startup command in its worktree. The server runs
  // it off-thread and routes the busy/success/error toasts back over `/ws`, so
  // this resolves as soon as the run is accepted (a non-2xx still throws).
  rerunStartupCommand: (id: string) =>
    request<void>(
      "POST",
      `/api/v1/sessions/${encodeURIComponent(id)}/rerun-startup-command`,
    ),
  // List the agent's startup-command log files (newest first) with the newest
  // file's contents pre-loaded for immediate display.
  startupLogs: (id: string) =>
    request<StartupLogsList>(
      "GET",
      `/api/v1/sessions/${encodeURIComponent(id)}/startup-logs`,
    ),
  // Read one startup-command log file by name (empty name returns the newest).
  startupLogContent: (id: string, name?: string) =>
    request<StartupLogContent>(
      "GET",
      `/api/v1/sessions/${encodeURIComponent(id)}/startup-logs/content${
        name ? `?name=${encodeURIComponent(name)}` : ""
      }`,
    ),
}
