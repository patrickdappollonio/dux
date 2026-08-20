import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { git } from "./git"
import { FileApiError, FileConflictError, fileApi } from "./fileApi"
import { agentRoot } from "@/lib/editorRoot"

// The git/file mutation clients are nested under the session resource: the
// session id is the `:id` path segment (`/api/v1/sessions/:id/git/*` and
// `/api/v1/sessions/:id/files/*`), and is no longer a body field. These assert
// the URLs the clients hit and the bodies they send.

const fetchMock = vi.fn(async () => {
  return {
    ok: true,
    status: 200,
    json: async () => ({ path: "a.txt", binary: false, content: "x" }),
    text: async () => "",
    headers: { get: () => null },
  } as unknown as Response
})

beforeEach(() => {
  vi.stubGlobal("fetch", fetchMock)
})

afterEach(() => {
  vi.unstubAllGlobals()
  vi.clearAllMocks()
})

describe("git REST client targets /api/v1/sessions/:id/git/*", () => {
  it("stage POSTs the nested path with a body-less session id", async () => {
    await git.stage("s1", "a.txt")
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/sessions/s1/git/stage",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ path: "a.txt" }),
      }),
    )
  })

  it("push POSTs the nested path with an empty body", async () => {
    await git.push("s1")
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/sessions/s1/git/push",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({}),
      }),
    )
  })

  it("pull POSTs the nested path with an empty body", async () => {
    await git.pull("s1")
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/sessions/s1/git/pull",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({}),
      }),
    )
  })

  it("commit POSTs the nested path with only the typed message", async () => {
    await git.commit("s1", "msg")
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/sessions/s1/git/commit",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ message: "msg" }),
      }),
    )
  })

  it("encodes the session id into the path", async () => {
    await git.stage("a/b c", "x.txt")
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/sessions/a%2Fb%20c/git/stage",
      expect.objectContaining({ method: "POST" }),
    )
  })
})

describe("file REST client targets /api/v1/sessions/:id/files/*", () => {
  it("read POSTs the nested path with a body-less session id", async () => {
    await fileApi.read(agentRoot("s1"), "a.txt")
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/sessions/s1/files/read",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ path: "a.txt" }),
      }),
    )
  })

  it("list POSTs the nested path with an empty body", async () => {
    await fileApi.list(agentRoot("s1"))
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/sessions/s1/files/list",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({}),
      }),
    )
  })

  it("write POSTs the nested path with path + content only", async () => {
    await fileApi.write(agentRoot("s1"), "a.txt", "hello")
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/sessions/s1/files/write",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ path: "a.txt", content: "hello" }),
      }),
    )
  })

  // rawUrl is a pure URL builder (no fetch): the image preview pane hands it
  // straight to an <img src>. It must hit the same GET /files/raw route the
  // markdown preview's asset proxy uses, with both segments encoded.
  it("rawUrl builds the GET /files/raw URL with encoded session id and path", () => {
    expect(fileApi.rawUrl(agentRoot("s1"), "img/logo.png")).toBe(
      "/api/v1/sessions/s1/files/raw?path=img%2Flogo.png",
    )
    expect(fileApi.rawUrl(agentRoot("a/b c"), "sp ace/ñ.png")).toBe(
      "/api/v1/sessions/a%2Fb%20c/files/raw?path=sp%20ace%2F%C3%B1.png",
    )
    expect(fetchMock).not.toHaveBeenCalled()
  })
})

// The save's freshness guard. The token is opt-in on the wire, and a refusal
// comes back as a typed error rather than as a status code the caller has to
// re-parse: the editor routes a conflict to a choice dialog, and everything
// else to the plain error toast.
describe("the guarded save", () => {
  it("omits the token entirely when the caller has none", async () => {
    await fileApi.write(agentRoot("s1"), "a.txt", "hello")
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/sessions/s1/files/write",
      expect.objectContaining({
        body: JSON.stringify({ path: "a.txt", content: "hello" }),
      }),
    )
  })

  it("sends both halves of the token when it has both", async () => {
    await fileApi.write(agentRoot("s1"), "a.txt", "hello", {
      modified: "2026-01-01T00:00:00+00:00",
      size: 5,
    })
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/sessions/s1/files/write",
      expect.objectContaining({
        body: JSON.stringify({
          path: "a.txt",
          content: "hello",
          expected_modified: "2026-01-01T00:00:00+00:00",
          expected_size: 5,
        }),
      }),
    )
  })

  it("sends no token at all when only one half is known", async () => {
    await fileApi.write(agentRoot("s1"), "a.txt", "hello", { modified: null, size: 5 })
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/sessions/s1/files/write",
      expect.objectContaining({
        body: JSON.stringify({ path: "a.txt", content: "hello" }),
      }),
    )
  })

  it("turns a 409 into a FileConflictError carrying the current stamp", async () => {
    fetchMock.mockResolvedValueOnce({
      ok: false,
      status: 409,
      clone: () => ({
        json: async () => ({
          modified: "2026-02-02T00:00:00+00:00",
          size: 42,
          deleted: false,
        }),
      }),
      json: async () => ({}),
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response)
    const err = await fileApi
      .write(agentRoot("s1"), "a.txt", "hello", { modified: "old", size: 5 })
      .catch((e: unknown) => e)
    expect(err).toBeInstanceOf(FileConflictError)
    const conflict = err as FileConflictError
    expect(conflict.status).toBe(409)
    expect(conflict.modified).toBe("2026-02-02T00:00:00+00:00")
    expect(conflict.size).toBe(42)
    expect(conflict.deleted).toBe(false)
  })

  it("reports a deleted-underneath refusal as its own rung", async () => {
    fetchMock.mockResolvedValueOnce({
      ok: false,
      status: 409,
      clone: () => ({
        json: async () => ({ modified: null, size: null, deleted: true }),
      }),
      json: async () => ({}),
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response)
    const err = (await fileApi
      .write(agentRoot("s1"), "a.txt", "hello", { modified: "old", size: 5 })
      .catch((e: unknown) => e)) as FileConflictError
    expect(err.deleted).toBe(true)
    expect(String(err.message)).toContain("deleted")
  })

  it("falls back to a plain error when a 409 body cannot be read", async () => {
    fetchMock.mockResolvedValueOnce({
      ok: false,
      status: 409,
      clone: () => ({
        json: async () => {
          throw new Error("not json")
        },
      }),
      json: async () => ({}),
      text: async () => "something went wrong",
      headers: { get: () => null },
    } as unknown as Response)
    const err = (await fileApi
      .write(agentRoot("s1"), "a.txt", "hello", { modified: "old", size: 5 })
      .catch((e: unknown) => e)) as FileApiError
    expect(err).not.toBeInstanceOf(FileConflictError)
    expect(err.status).toBe(409)
    expect(err.message).toBe("something went wrong")
  })
})
