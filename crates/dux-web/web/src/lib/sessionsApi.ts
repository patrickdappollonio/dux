// Scoped REST client for agent-session mutations. Requests include the
// connection id so server-side operation status is routed back to the
// initiating client; failures surface as `SessionsApiError`.

import { createJsonRequest } from "./jsonRequest"
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
  | { kind: "from_pr"; project_id: string; pr: string; name?: string }
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
}

// The answer to `GET /api/v1/sessions/:id/branch-unpushed`. `branches` is every
// branch the delete would remove (two for a drifted agent) and the count is
// their union, so the dialog can name exactly what would go.
export type BranchUnpushed = {
  branches: string[]
  // `null` when git could not answer. The count travels with the has-remote flag
  // because it means a different thing without it: with nowhere to have pushed to,
  // the count is simply the whole history.
  unpushed: { count: number; has_remote_refs: boolean } | null
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
  // `deleteBranch` is the delete dialog's "also delete the branch" answer.
  // `null` means nobody was asked (a standalone agent, whose dialog has no
  // checkbox), and the server then keeps its provenance default.
  remove: (id: string, deleteWorktree: boolean, deleteBranch: boolean | null) =>
    request<void>(
      "DELETE",
      `/api/v1/sessions/${encodeURIComponent(id)}?delete_worktree=${deleteWorktree}` +
        (deleteBranch === null ? "" : `&delete_branch=${deleteBranch}`),
    ),
  // How much work ticking that box would destroy: the branches the delete removes and
  // how many of their commits no remote-tracking ref reaches. A `null` count means git
  // could not answer, and the dialog then says nothing rather than guessing.
  branchUnpushed: (id: string) =>
    request<BranchUnpushed>(
      "GET",
      `/api/v1/sessions/${encodeURIComponent(id)}/branch-unpushed`,
    ),
  patch: (id: string, body: PatchSessionBody) =>
    request<{ provider_change?: string }>(
      "PATCH",
      `/api/v1/sessions/${encodeURIComponent(id)}`,
      body,
    ),
  reconnect: (id: string, force: boolean) =>
    request<void>("POST", `/api/v1/sessions/${encodeURIComponent(id)}/reconnect`, {
      force,
    }),
  // Force-kill the agent's running PTY (it detaches; it is NOT deleted). Used by
  // the kill-running modal. A non-2xx throws.
  kill: (id: string) =>
    request<void>("POST", `/api/v1/sessions/${encodeURIComponent(id)}/kill`),
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
  reorder: (projectId: string, sessionIds: string[]) =>
    request<void>("POST", "/api/v1/sessions/reorder", {
      project_id: projectId,
      session_ids: sessionIds,
    }),
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
