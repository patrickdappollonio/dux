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
// A non-2xx is thrown as a `SpineFetchError` carrying the HTTP status so the
// caller can branch.

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
  /** Every known project, in display order. */
  projects: ProjectView[]
  /** Every agent session, in display order. */
  sessions: SessionView[]
  /** EVERY companion terminal, of every owner, as one flat collection ordered by
   * the manual `sort_order`. Each entry carries its own tagged `owner`, so the
   * client no longer rebuilds this list by walking two nested collections and
   * inferring ownership from which one it was in. An older server that predates
   * the flat shape omits the field; `fetchSpine` normalizes it to `[]`. */
  terminals: TerminalView[]
  /** Core-computed sidebar grouping (projects + sessions, orphans surfaced) so
   * both surfaces render an identical tree without re-deriving grouping. */
  sidebar: SidebarModel
}

// A failed spine fetch. `status` is the HTTP status (0 for a network/transport
// failure with no response). The boot path swallows this and keeps the
// last-known spine (null on first boot); a later `projects.changed` /
// `sessions.changed` event or a reconnect retries.
export class SpineFetchError extends Error {
  readonly status: number

  constructor(message: string, status: number) {
    super(message)
    this.name = "SpineFetchError"
    this.status = status
  }
}

export async function fetchSpine(): Promise<Spine> {
  let resp: Response
  try {
    resp = await fetch("/api/v1/spine", { credentials: "same-origin" })
  } catch {
    // The request never reached the server (offline, DNS, CORS).
    throw new SpineFetchError("Could not reach the server.", 0)
  }
  if (!resp.ok) {
    const detail = (await resp.text().catch(() => "")).trim()
    throw new SpineFetchError(
      detail || `request failed (${resp.status})`,
      resp.status,
    )
  }
  // Coerce optional-on-the-wire session fields to their required shapes at the
  // single ingestion boundary. An older server (e.g. after a binary downgrade,
  // seen by an already-open client) omits `tabs`, `initial_branch`, and
  // `source_branch`, but every downstream consumer treats them as required: `tabs`
  // becomes `[]` and the two branch fields become `""` (falsy, so the
  // "Unknown"/no-drift fallbacks in the info dialog and header still apply).
  const raw = (await resp.json()) as Omit<
    Spine,
    "sessions" | "projects" | "terminals"
  > & {
    projects: ProjectView[]
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
      }
    >
  }
  return {
    ...raw,
    // An older server that predates the flat, owner-bearing collection omits the
    // field; every downstream consumer treats it as required, so normalize to
    // `[]` here (that server nested its terminals instead, and a client running
    // against it simply shows none, which is the same thing the pre-existing
    // `?? []` fallbacks did for a missing nested list).
    terminals: (raw.terminals ?? []).map(normalizeTerminal),
    sessions: raw.sessions.map((s) => ({
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
