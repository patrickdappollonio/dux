import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { git } from "./git"
import { fileApi } from "./fileApi"

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
    await fileApi.read("s1", "a.txt")
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/sessions/s1/files/read",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ path: "a.txt" }),
      }),
    )
  })

  it("list POSTs the nested path with an empty body", async () => {
    await fileApi.list("s1")
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/sessions/s1/files/list",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({}),
      }),
    )
  })

  it("write POSTs the nested path with path + content only", async () => {
    await fileApi.write("s1", "a.txt", "hello")
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
    expect(fileApi.rawUrl("s1", "img/logo.png")).toBe(
      "/api/v1/sessions/s1/files/raw?path=img%2Flogo.png",
    )
    expect(fileApi.rawUrl("a/b c", "sp ace/ñ.png")).toBe(
      "/api/v1/sessions/a%2Fb%20c/files/raw?path=sp%20ace%2F%C3%B1.png",
    )
    expect(fetchMock).not.toHaveBeenCalled()
  })
})
