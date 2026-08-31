// HTTP client for the workspace "spine": the projects, sessions, and core-computed
// sidebar grouping. Like `bootstrapApi.ts` and `changesApi.ts`, this is a plain
// GET (the read-only `git.ts` pattern, with `credentials: "same-origin"`) so it
// composes with HTTP caching and reads as a resource fetch. The matching
// `projects.changed` / `sessions.changed` events over `/ws/events` tell the
// client WHEN to re-GET.
//
// The authoritative document projects live projects, sessions, terminals, and
// the core sidebar model into one on-demand REST response.
// A non-2xx is thrown as a `WorkspaceFetchError` carrying the HTTP status so the
// caller can branch.
//
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
  /** The server's monotonic revision of this document, minted where the server
   * rebuilds its cached serialization. It is embedded in the document itself,
   * so a document that arrived over REST and one that arrived as a push frame
   * are orderable against each other. Optional because a server that predates
   * the push does not send it; an absent rev means "not orderable", and the
   * client applies such a document rather than guessing. Meaningless across
   * server restarts, which is why the client forgets what it applied whenever
   * its events socket reopens. */
  rev?: number
  /** Every known project, in display order. */
  projects: ProjectView[]
  /** Every agent session, in display order. */
  sessions: SessionView[]
  /** EVERY companion terminal, of every owner, as one flat collection ordered by
   * the manual `sort_order`. Each entry carries its own tagged `owner`, so the
   * client does not infer ownership by walking nested collections. An older server that predates
   * the flat shape nests its terminals instead; `fetchWorkspace` flattens those and
   * tags each with the owner it was nested under (see `ingestTerminals`). */
  terminals: TerminalView[]
  /** Core-computed sidebar grouping (projects + sessions, orphans surfaced) so
   * both surfaces render an identical tree without re-deriving grouping. */
  sidebar: SidebarModel
}

// A failed spine fetch. `status` is the HTTP status (0 for a network/transport
// failure with no response). The boot path swallows this and keeps the
// last-known spine (null on first boot); a later `projects.changed` /
// `sessions.changed` event or a reconnect retries.
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
 * optional-on-the-wire fields still optional, terminals possibly still nested
 * inside their owners. Both delivery paths hand this to
 * [`normalizeWorkspace`]. */
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
       * standalone agent, which sent the git fields flat beside the session
       * instead; `normalizeWorkspace` synthesizes the managed shape from
       * those, which is exactly right, because every agent such a server can
       * have IS managed. */
      workspace?: AgentWorkspaceWire
      /** The legacy FLAT git fields. Present only from a server that predates
       * the tagged workspace, and read only by the synthesis below. Nothing
       * downstream may reach for them: an agent with no branch has no honest
       * value to put here, which is why they moved inside the tag. */
      project_id?: string
      branch_name?: string
      initial_branch?: string
      source_branch?: string
      worktree_path?: string
      branch_provenance?: "created" | "attached" | "adopted" | "unknown"
    }
  >
}

/** THE ONE NORMALIZATION POINT for an agent's workspace, in both directions.
 *
 * BACKWARD compatibility: an older server sends the git fields flat, so the
 * managed shape is synthesized from them. That synthesis is safe precisely
 * because a server old enough to send them is one where every agent is managed,
 * so there is no folder case to get wrong.
 *
 * FORWARD compatibility is also here, and deliberately NOT in the matcher. A
 * NEWER server could send a third kind, and `matchWorkspace` throws on a kind it
 * has never heard of; that throw is what keeps a missing case a compile error,
 * but it runs inside render paths, so an unknown kind reaching a component
 * unmounts the whole React root. Degrading once, at ingestion, keeps the
 * matcher's exhaustiveness guarantee and turns "a kind from the future" into one
 * odd-looking agent instead of a blank page. The direction is the same as the
 * absent case below: unknown reads as managed, because telling the delete dialog
 * a directory is the user's own when dux may in fact own that worktree is the
 * wrong way to be wrong.
 *
 * The absent-and-not-legacy case (neither a workspace nor a branch name, which
 * no real server produces) still yields a managed shape with empty fields
 * rather than a folder: reading an unclassifiable agent as a folder would tell
 * the delete dialog its directory is the user's and must be kept, which is the
 * wrong direction to be wrong in for a worktree dux may actually own. */
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
    // An older server is one that deletes the branch either way, so a missing
    // provenance reads as "created": the copy must describe what the server it
    // is talking to will actually do.
    branch_provenance: raw.branch_provenance ?? "created",
    source_branch: raw.source_branch ?? "",
    worktree_path: raw.worktree_path ?? "",
  }
}

// Turn a wire document into the shape every consumer downstream assumes.
//
// This is the ONE ingestion boundary, and it is shared deliberately: the same
// document reaches the client two ways (fetched at boot and on recovery, pushed
// on every change), and two normalizers would drift the moment either wire
// shape grew a field. Pure and total: it reads the raw document and returns the
// normalized one, touching nothing else, so it can be called from the socket
// handler as safely as from the fetch.
//
// Coerce optional-on-the-wire session fields to their required shapes here. An
// older server (e.g. after a binary downgrade, seen by an already-open client)
// omits `tabs`, `initial_branch`, and `source_branch`, but every downstream
// consumer treats them as required: `tabs` becomes `[]` and the two branch
// fields become `""` (falsy, so the "Unknown"/no-drift fallbacks in the info
// dialog and header still apply).
export function normalizeWorkspace(raw: RawWorkspace): Spine {
  return {
    ...raw,
    terminals: ingestTerminals(raw),
    // The nested arrays, when an older server sent them, are dropped from the
    // owner they rode on: `ingestTerminals` has already lifted them into the one
    // flat collection, and leaving a second, staler copy behind invites a
    // consumer to read it.
    projects: raw.projects.map(({ terminals: _nested, ...p }) => p),
    sessions: raw.sessions.map((rawSession) => {
      const {
        terminals: _nested,
        // The legacy flat git fields are DROPPED here, not merely superseded:
        // they have been folded into the tagged workspace above, and leaving a
        // second, flatter copy behind is an invitation to read it.
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
        // An older server that predates the published slot pointer omits the
        // field. Such a server keeps the first tab's id equal to the session id,
        // so that is exactly what it meant; filling it here means every consumer
        // reads one required field instead of re-deriving the rule.
        slot_tab_id: s.slot_tab_id ?? s.id,
      }
    }),
  }
}

// The `typing` cue is newer than the tab/terminal views themselves, so an older
// server omits it on nested tabs and terminals. Normalize each to a required
// `false` at this single ingestion boundary, matching how the session-level
// fields above are coerced (downstream consumers treat `typing`/`working` as
// required).
type RawTab = Omit<AgentTabView, "typing"> & { typing?: boolean }
// The sort keys (`sort_order`/`created_at`/`updated_at`) are newer than the
// terminal view too, added for the terminal-sort/drag parity work; an older
// server omits them, so coerce each to a safe default at this same boundary
// (`sort_order` to 0, the timestamps to "", which the pure sort treats as epoch 0).
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

// A terminal as an OLDER server sent it: nested inside its owner, and carrying
// no owner of its own, because the collection it sat in was the ownership.
type LegacyTerminal = Omit<RawTerminal, "owner">

// The flat, owner-bearing collection, from EITHER shape the server may send.
//
// The two shapes belong to two builds of dux, and a browser tab left open while
// dux restarts (entirely ordinary during development) is how one client meets
// both. A new server sends `terminals` at the top level, each entry tagged with
// its owner. An older one omits that field and nests terminals inside the
// session or the project that owns them instead, so ownership is which array a
// terminal was found in; those are lifted out and tagged here, at the single
// ingestion boundary, rather than being discarded. Discarding them would show
// that user an empty Terminals section with every terminal still running.
//
// The flat field wins whenever it is present, even when empty: a new server with
// no terminals sends `[]` and means it.
//
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
  // The flat collection promises the global `sort_order` base order, which the
  // nested arrays only held WITHIN each owner. `Array.prototype.sort` is stable,
  // so terminals sharing a `sort_order` keep the order they were nested in.
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
