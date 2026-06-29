import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { git } from "./git"
import { fileApi } from "./fileApi"

// Phase 6 unifies the git/file mutation clients onto the versioned `/api/v1/*`
// prefix (the unversioned `/api/git/*` and `/api/file/*` aliases were retired
// server-side). These assert the URLs the clients actually hit.

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

describe("git REST client targets /api/v1/git/*", () => {
  it("stage POSTs /api/v1/git/stage", async () => {
    await git.stage("s1", "a.txt")
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/git/stage",
      expect.objectContaining({ method: "POST" }),
    )
  })

  it("push POSTs /api/v1/git/push", async () => {
    await git.push("s1")
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/git/push",
      expect.objectContaining({ method: "POST" }),
    )
  })

  it("commit POSTs /api/v1/git/commit with the typed message", async () => {
    await git.commit("s1", "msg")
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/git/commit",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ session_id: "s1", message: "msg" }),
      }),
    )
  })
})

describe("file REST client targets /api/v1/file/*", () => {
  it("read POSTs /api/v1/file/read", async () => {
    await fileApi.read("s1", "a.txt")
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/file/read",
      expect.objectContaining({ method: "POST" }),
    )
  })

  it("list POSTs /api/v1/file/list", async () => {
    await fileApi.list("s1")
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/file/list",
      expect.objectContaining({ method: "POST" }),
    )
  })
})
