// HTTP client for the workspace "spine": live projects, sessions, terminals and
// the core-computed sidebar model in one GET. The `projects.changed` /
// `sessions.changed` events over `/ws/events` tell the client when to re-GET,
// and a non-2xx throws a `WorkspaceFetchError` carrying the HTTP status.
import type { AgentWorkspaceWire } from "@/lib/agentWorkspace"
import type {
  AgentTabView,
  ProjectView,
  SessionView,
  SidebarModel,
  TerminalView,
} from "./types"

// The spine document mirrors the server's JSON.
export interface Spine {
  /** The server's monotonic revision of this document, so a document fetched
   * over REST and one pushed over the socket are orderable against each other.
   * Absent from a server that predates the push, which means "not orderable":
   * the client applies such a document rather than guessing. Meaningless across
   * server restarts. */
  rev?: number
  /** Every known project, in display order. */
  projects: ProjectView[]
  /** Every agent session, in display order. */
  sessions: SessionView[]
  /** Every companion terminal, of every owner, as one flat collection ordered by
   * the manual `sort_order`. Each entry carries its own tagged `owner`, so
   * ownership is never inferred from the collection an entry was found in. */
  terminals: TerminalView[]
  /** Core-computed sidebar grouping (projects + sessions, orphans surfaced) so
   * both surfaces render an identical tree without re-deriving grouping. */
  sidebar: SidebarModel
}

// A failed spine fetch. `status` is the HTTP status, 0 for a transport failure
// with no response at all.
export class WorkspaceFetchError extends Error {
  readonly status: number

  constructor(message: string, status: number) {
    super(message)
    this.name = "WorkspaceFetchError"
    this.status = status
  }
}

export async function fetchWorkspace(): Promise<Spine> {
  let resp: Response
  try {
    resp = await fetch("/api/v1/workspace", { credentials: "same-origin" })
  } catch {
    // The request never reached the server (offline, DNS, CORS).
    throw new WorkspaceFetchError("Could not reach the server.", 0)
  }
  if (!resp.ok) {
    const detail = (await resp.text().catch(() => "")).trim()
    throw new WorkspaceFetchError(
      detail || `request failed (${resp.status})`,
      resp.status,
    )
  }
  return normalizeWorkspace((await resp.json()) as RawWorkspace)
}

/** The workspace document as it arrives on the wire, from any server version:
 * optional fields still optional, terminals possibly still nested inside their
 * owners. Both delivery paths hand this to `normalizeWorkspace`. */
export type RawWorkspace = Omit<
  Spine,
  "sessions" | "projects" | "terminals"
> & {
  projects: Array<ProjectView & { terminals?: LegacyTerminal[] }>
  terminals?: RawTerminal[]
  sessions: Array<
    Omit<
      SessionView,
      | "tabs"
      | "needs_attention"
      | "typing"
      | "last_focused_tab"
      | "workspace"
      | "slot_tab_id"
    > & {
      tabs?: RawTab[]
      slot_tab_id?: string
      needs_attention?: boolean
      typing?: boolean
      last_focused_tab?: string | null
      terminals?: LegacyTerminal[]
      /** The tagged workspace. Absent from a server that predates the
       * standalone agent, whose agents are all managed. */
      workspace?: AgentWorkspaceWire
      /** The flat git fields an older server sends instead, read only by the
       * synthesis below. Nothing downstream may reach for them: an agent with
       * no branch has no honest value to put here. */
      project_id?: string
      branch_name?: string
      initial_branch?: string
      source_branch?: string
      worktree_path?: string
      branch_provenance?: "created" | "attached" | "adopted" | "unknown"
    }
  >
}

/** The one normalization point for an agent's workspace, in both directions.
 *
 * Anything not recognizably a folder reads as managed: an older server's flat
 * git fields, a kind from a newer server, and the unclassifiable case alike.
 * Telling the delete dialog a directory is the user's own when dux may in fact
 * own that worktree is the wrong way to be wrong.
 *
 * A kind from the future is degraded here rather than in `matchWorkspace`, whose
 * throw is what keeps a missing case a compile error but runs in render paths,
 * where an unknown kind would unmount the React root. */
function normalizeSessionWorkspace(
  raw: RawWorkspace["sessions"][number],
): AgentWorkspaceWire {
  if (raw.workspace) {
    if (raw.workspace.kind === "managed" || raw.workspace.kind === "folder") {
      return raw.workspace
    }
    // A kind from a newer server. Read as the managed shape it may well be a
    // superset of, with whatever fields it does carry.
    const unknown = raw.workspace as Record<string, unknown>
    const str = (key: string) =>
      typeof unknown[key] === "string" ? (unknown[key] as string) : ""
    return {
      kind: "managed",
      project_id: str("project_id"),
      branch_name: str("branch_name"),
      initial_branch: str("initial_branch"),
      branch_provenance: "unknown",
      source_branch: str("source_branch"),
      worktree_path: str("worktree_path"),
    }
  }
  return {
    kind: "managed",
    project_id: raw.project_id ?? "",
    branch_name: raw.branch_name ?? "",
    initial_branch: raw.initial_branch ?? "",
    // A server old enough to omit provenance deletes the branch either way, so
    // "created" is what the copy must promise the user.
    branch_provenance: raw.branch_provenance ?? "created",
    source_branch: raw.source_branch ?? "",
    worktree_path: raw.worktree_path ?? "",
  }
}

// Turn a wire document into the shape every consumer downstream assumes.
//
// The one ingestion boundary, shared by both delivery paths (the boot fetch and
// the push frame) so the two cannot drift. Pure and total, touching nothing
// else, so the socket handler may call it as safely as the fetch. Fields an
// older server may omit are coerced to the required shapes downstream assumes.
export function normalizeWorkspace(raw: RawWorkspace): Spine {
  return {
    ...raw,
    terminals: ingestTerminals(raw),
    // `ingestTerminals` has lifted any nested array into the flat collection;
    // the copy on the owner is dropped so nothing reads the staler one.
    projects: raw.projects.map(({ terminals: _nested, ...p }) => p),
    sessions: raw.sessions.map((rawSession) => {
      const {
        terminals: _nested,
        // Folded into the tagged workspace above, and dropped here so nothing
        // downstream reads the flat copy.
        project_id: _projectId,
        branch_name: _branchName,
        initial_branch: _initialBranch,
        source_branch: _sourceBranch,
        worktree_path: _worktreePath,
        branch_provenance: _branchProvenance,
        ...s
      } = rawSession
      return {
        ...s,
        workspace: normalizeSessionWorkspace(rawSession),
        tabs: (s.tabs ?? []).map(normalizeTab),
        // An older server that predates attention omits the field; treat missing
        // as "no attention" so the dot/count/favicon stay quiet.
        needs_attention: s.needs_attention ?? false,
        // An older server that predates the finer "typing" cue omits it; treat
        // missing as "not typing" so the row stays on the working/idle words.
        typing: s.typing ?? false,
        // An older server that predates tab-focus memory omits the field; treat
        // missing the same as an explicit null ("no memory recorded").
        last_focused_tab: s.last_focused_tab ?? null,
        // The session id is the placeholder for "this agent's first tab,
        // whichever it is" (see `slotTabTargetId`), so a server that omits the
        // pointer still leaves every consumer one required field to read.
        slot_tab_id: s.slot_tab_id ?? s.id,
      }
    }),
  }
}

// An older server omits `typing`, which downstream consumers treat as required.
type RawTab = Omit<AgentTabView, "typing"> & { typing?: boolean }
// An older server omits the sort keys too. The timestamps default to "", which
// the pure sort treats as epoch 0.
type RawTerminal = Omit<
  TerminalView,
  "working" | "typing" | "sort_order" | "created_at" | "updated_at"
> & {
  working?: boolean
  typing?: boolean
  sort_order?: number
  created_at?: string
  updated_at?: string
}

// A terminal as an older server sends it: nested inside its owner, and carrying
// no owner of its own, because the collection it sits in is the ownership.
type LegacyTerminal = Omit<RawTerminal, "owner">

// The flat, owner-bearing collection, from either shape the server may send. A
// newer server sends `terminals` at the top level, each entry tagged with its
// owner; an older one nests them inside the owning session or project, and those
// are lifted and tagged here rather than discarded, which would show an empty
// Terminals section while every terminal is still running.
//
// The flat field wins whenever it is present, even when empty: a server with no
// terminals sends `[]` and means it.
function ingestTerminals(raw: {
  terminals?: RawTerminal[]
  sessions: ReadonlyArray<{ id: string; terminals?: LegacyTerminal[] }>
  projects: ReadonlyArray<{ id: string; terminals?: LegacyTerminal[] }>
}): TerminalView[] {
  if (raw.terminals) return raw.terminals.map(normalizeTerminal)
  const nested: TerminalView[] = []
  for (const session of raw.sessions) {
    for (const t of session.terminals ?? []) {
      nested.push(
        normalizeTerminal({
          ...t,
          owner: { kind: "session", session_id: session.id },
        }),
      )
    }
  }
  for (const project of raw.projects) {
    for (const t of project.terminals ?? []) {
      nested.push(
        normalizeTerminal({
          ...t,
          owner: { kind: "project", project_id: project.id },
        }),
      )
    }
  }
  // The flat collection promises a global `sort_order` order, which the nested
  // arrays hold only within each owner. The sort is stable, so terminals sharing
  // a `sort_order` keep the order they were nested in.
  return nested.sort((a, b) => a.sort_order - b.sort_order)
}

function normalizeTab(t: RawTab): AgentTabView {
  return { ...t, typing: t.typing ?? false }
}

function normalizeTerminal(t: RawTerminal): TerminalView {
  return {
    ...t,
    working: t.working ?? false,
    typing: t.typing ?? false,
    sort_order: t.sort_order ?? 0,
    created_at: t.created_at ?? "",
    updated_at: t.updated_at ?? "",
  }
}
