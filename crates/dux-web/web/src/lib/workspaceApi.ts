// HTTP client for the workspace "spine": the projects, sessions, and core-computed
// sidebar grouping. Like `bootstrapApi.ts` and `changesApi.ts`, this is a plain
// GET (the read-only `git.ts` pattern, with `credentials: "same-origin"`) so it
// composes with HTTP caching and reads as a resource fetch. The matching
// `projects.changed` / `sessions.changed` events over `/ws/events` tell the
// client WHEN to re-GET.
//
// These three fields used to ride the broadcast `ViewModel`; they are volatile
// workspace state but are now read on demand over REST rather than re-broadcast
// to every client on every change. The server is authoritative: it projects the
// live projects/sessions plus the core sidebar model into this single document.
// A non-2xx is thrown as a `WorkspaceFetchError` carrying the HTTP status so the
// caller can branch.
//
// The URL, this module, and the fetch are named for the workspace; the in-code
// TYPE is still `Spine`, matching the server's `SpineView`. The wire and the
// route are the user-facing halves and they were renamed; renaming the internal
// types on both sides would be churn without user value, so the two languages
// keep the same internal word deliberately rather than by omission.

import type {
  AgentTabView,
  ProjectView,
  SessionView,
  SidebarModel,
  TerminalView,
} from "./types"

// The spine document. Field names/types mirror the server's JSON and the values
// the legacy ViewModel carried, so consumers move over without a shape change.
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
   * client no longer rebuilds this list by walking two nested collections and
   * inferring ownership from which one it was in. An older server that predates
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
      | "initial_branch"
      | "source_branch"
      | "needs_attention"
      | "typing"
      | "last_focused_tab"
    > & {
      tabs?: RawTab[]
      initial_branch?: string
      source_branch?: string
      needs_attention?: boolean
      typing?: boolean
      last_focused_tab?: string | null
      terminals?: LegacyTerminal[]
    }
  >
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
    sessions: raw.sessions.map(({ terminals: _nested, ...s }) => ({
      ...s,
      tabs: (s.tabs ?? []).map(normalizeTab),
      initial_branch: s.initial_branch ?? "",
      source_branch: s.source_branch ?? "",
      // An older server that predates attention omits the field; treat missing
      // as "no attention" so the dot/count/favicon stay quiet.
      needs_attention: s.needs_attention ?? false,
      // An older server that predates the finer "typing" cue omits it; treat
      // missing as "not typing" so the row stays on the working/idle words.
      typing: s.typing ?? false,
      // An older server that predates tab-focus memory omits the field; treat
      // missing the same as an explicit null ("no memory recorded").
      last_focused_tab: s.last_focused_tab ?? null,
    })),
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
// The OTHER direction cannot be repaired from here: a client old enough to want
// nested arrays, talking to a server that no longer sends them, sees no
// terminals, and dux has no mechanism that would make such a tab reload. The
// only reload path in the client is reactive, `ChunkBoundary` catching a failed
// lazy-chunk import after the assets it wants have gone.
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
