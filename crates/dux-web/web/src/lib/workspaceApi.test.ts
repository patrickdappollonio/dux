import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { WorkspaceFetchError, fetchWorkspace } from "./workspaceApi"

// The spine client is a thin GET wrapper (mirrors bootstrapApi/changesApi): on
// 2xx it returns the parsed JSON (coercing each session's `tabs` to an array so
// an older server that omits the field degrades safely); on a non-2xx it throws a
// `WorkspaceFetchError` carrying the HTTP status; on a transport failure it throws 0.

beforeEach(() => {
  vi.restoreAllMocks()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe("fetchWorkspace", () => {
  it("issues a same-origin GET and returns the parsed body on 200", async () => {
    const body = {
      projects: [{ id: "p1" }],
      sessions: [{ id: "s1", project_id: "p1" }],
      sidebar: { groups: [], agentless_start: null },
    }
    const fetchMock = vi.fn(async () => ({
      ok: true,
      status: 200,
      json: async () => body,
      text: async () => "",
      headers: { get: () => null },
    })) as unknown as typeof fetch
    vi.stubGlobal("fetch", fetchMock)

    const result = await fetchWorkspace()
    // A session that omits `tabs`/`initial_branch`/`source_branch`/
    // `needs_attention`/`last_focused_tab` (an older server) is coerced to
    // `tabs: []`, empty-string branch fields, `needs_attention: false`, and
    // `last_focused_tab: null`.
    expect(result).toEqual({
      ...body,
      // This body carries no terminals in EITHER shape (no flat collection and
      // no nested arrays), so the flat collection is empty. A body that nests
      // them is a different case, covered below.
      terminals: [],
      sessions: [
        {
          id: "s1",
          workspace: {
            kind: "managed",
            project_id: "p1",
            branch_name: "",
            initial_branch: "",
            branch_provenance: "created",
            source_branch: "",
            worktree_path: "",
          },
          tabs: [],
          // A session that omits `typing` (an older server) is coerced to
          // `typing: false`.
          typing: false,
          needs_attention: false,
          last_focused_tab: null,
          // A session that omits `slot_tab_id` (an older server) is coerced to
          // the session id, the placeholder for "the first tab, whichever it is".
          slot_tab_id: "s1",
        },
      ],
    })
    expect(fetchMock).toHaveBeenCalledWith("/api/v1/workspace", {
      credentials: "same-origin",
    })
  })

  // Leaving a browser tab open while dux restarts is ordinary, so a client can
  // outlive the server that served it and end up talking to a build that sends
  // the OTHER shape. A server predating the flat collection nests each terminal
  // inside its owner and puts no owner on the terminal itself, so the ingestion
  // boundary flattens those and attaches the owner they were nested under.
  // Reading only the flat field and normalizing a missing one to `[]` would show
  // that user no terminals at all, which is not what the old client did with the
  // very same body.
  it("flattens an older server's nested terminals and attaches the owner they were nested under", async () => {
    const body = {
      projects: [
        {
          id: "p1",
          terminals: [{ id: "t-p1", label: "Terminal 2", sort_order: 2 }],
        },
      ],
      sessions: [
        {
          id: "s1",
          workspace: {
            kind: "managed",
            project_id: "p1",
            branch_name: "",
            initial_branch: "",
            branch_provenance: "created",
            source_branch: "",
            worktree_path: "",
          },
          terminals: [
            { id: "t-s1a", label: "Terminal 1", sort_order: 1 },
            { id: "t-s1b", label: "Terminal 3", sort_order: 3 },
          ],
        },
      ],
      sidebar: { groups: [], agentless_start: null },
    }
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({
        ok: true,
        status: 200,
        json: async () => body,
        text: async () => "",
        headers: { get: () => null },
      })) as unknown as typeof fetch,
    )

    const result = await fetchWorkspace()
    // Flat, in the `sort_order` order the new shape promises, each carrying the
    // owner it was nested under.
    expect(result.terminals.map((t) => [t.id, t.owner])).toEqual([
      ["t-s1a", { kind: "session", session_id: "s1" }],
      ["t-p1", { kind: "project", project_id: "p1" }],
      ["t-s1b", { kind: "session", session_id: "s1" }],
    ])
    // Older servers omit the newer terminal fields as well; they are coerced
    // here exactly as a flat terminal's are.
    expect(result.terminals[0].working).toBe(false)
    expect(result.terminals[0].typing).toBe(false)
    // The nested arrays do not survive: the flat collection is the one place
    // terminals live, so nothing downstream can read a second, staler copy.
    expect(
      (result.sessions[0] as unknown as { terminals?: unknown }).terminals,
    ).toBeUndefined()
    expect(
      (result.projects[0] as unknown as { terminals?: unknown }).terminals,
    ).toBeUndefined()
  })

  // The flat collection wins when a body somehow carries both, so the owner-
  // tagged shape is never second-guessed by a nested leftover.
  it("prefers the flat collection and drops nested arrays when a body carries both", async () => {
    const body = {
      projects: [{ id: "p1", terminals: [{ id: "stale-p", label: "old" }] }],
      sessions: [
        { id: "s1", project_id: "p1", terminals: [{ id: "stale-s", label: "old" }] },
      ],
      terminals: [
        {
          id: "t-1",
          owner: { kind: "session", session_id: "s1" },
          label: "Terminal 1",
        },
      ],
      sidebar: { groups: [], agentless_start: null },
    }
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({
        ok: true,
        status: 200,
        json: async () => body,
        text: async () => "",
        headers: { get: () => null },
      })) as unknown as typeof fetch,
    )

    const result = await fetchWorkspace()
    expect(result.terminals.map((t) => t.id)).toEqual(["t-1"])
  })

  it("passes a session's last_focused_tab through verbatim when the server sends one", async () => {
    const body = {
      projects: [],
      sessions: [{ id: "s1", project_id: "p1", last_focused_tab: "tab-1" }],
      sidebar: { groups: [], agentless_start: null },
    }
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({
        ok: true,
        status: 200,
        json: async () => body,
        text: async () => "",
        headers: { get: () => null },
      })) as unknown as typeof fetch,
    )

    const result = await fetchWorkspace()
    expect(result.sessions[0].last_focused_tab).toBe("tab-1")
  })

  it("synthesizes the managed workspace from an older server's flat git fields", async () => {
    const body = {
      projects: [],
      // A session from an older server omits the two branch fields (and tabs).
      sessions: [{ id: "s1", project_id: "p1" }],
      sidebar: { groups: [], agentless_start: null },
    }
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({
        ok: true,
        status: 200,
        json: async () => body,
        text: async () => "",
        headers: { get: () => null },
      })) as unknown as typeof fetch,
    )

    const result = await fetchWorkspace()
    expect(result.sessions[0]).toMatchObject({
      id: "s1",
      workspace: {
        kind: "managed",
        project_id: "p1",
        branch_name: "",
        initial_branch: "",
        branch_provenance: "created",
        source_branch: "",
        worktree_path: "",
      },
      tabs: [],
    })
  })

  // The folder shape must survive ingestion untouched: the delete dialog, the
  // changes panel and every gate read it, and a normalizer that quietly
  // rewrote it would hand a standalone agent the managed rules.
  it("passes a folder workspace through ingestion unchanged", async () => {
    const workspace = {
      kind: "folder",
      folder_path: "/home/someone/notes",
      folder_label: "~/notes",
      repo_status: "working_repo",
      quiet_reason: "This folder is a git repository.",
    }
    const body = {
      projects: [],
      sessions: [{ id: "sa1", workspace }],
      sidebar: { groups: [], agentless_start: null },
    }
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({
        ok: true,
        status: 200,
        json: async () => body,
        text: async () => "",
        headers: { get: () => null },
      })) as unknown as typeof fetch,
    )

    const result = await fetchWorkspace()
    expect(result.sessions[0].workspace).toEqual(workspace)
  })

  // A kind from a NEWER server. The matcher throws on an unknown kind, and it
  // runs inside render paths, so ingestion degrades it here instead: one
  // odd-looking agent rather than a blank page. Managed rather than folder,
  // because telling the delete dialog a directory is the user's own when dux may
  // own that worktree is the wrong way to be wrong.
  it("degrades a workspace kind from the future to the managed shape", async () => {
    const body = {
      projects: [],
      sessions: [
        {
          id: "s9",
          workspace: {
            kind: "something-new",
            project_id: "p1",
            branch_name: "feature/x",
          },
        },
      ],
      sidebar: { groups: [], agentless_start: null },
    }
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({
        ok: true,
        status: 200,
        json: async () => body,
        text: async () => "",
        headers: { get: () => null },
      })) as unknown as typeof fetch,
    )

    const result = await fetchWorkspace()
    expect(result.sessions[0].workspace).toMatchObject({
      kind: "managed",
      project_id: "p1",
      branch_name: "feature/x",
      // Nothing is known about what such an agent's branch was, so the
      // provenance that means exactly that.
      branch_provenance: "unknown",
    })
  })

  it("throws a WorkspaceFetchError carrying the HTTP status on a non-2xx", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({
        ok: false,
        status: 503,
        json: async () => ({}),
        text: async () => "server starting",
        headers: { get: () => null },
      })) as unknown as typeof fetch,
    )
    await expect(fetchWorkspace()).rejects.toMatchObject({
      name: "WorkspaceFetchError",
      status: 503,
      message: "server starting",
    })
  })

  it("throws status 0 on a transport failure", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new Error("offline")
      }) as unknown as typeof fetch,
    )
    const err = await fetchWorkspace().catch((e) => e)
    expect(err).toBeInstanceOf(WorkspaceFetchError)
    expect(err.status).toBe(0)
  })
})
