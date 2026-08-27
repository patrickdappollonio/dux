import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { setConnectionId } from "./connection"
import { ProjectsApiError, projectsApi } from "./projectsApi"
import { SessionsApiError, sessionsApi } from "./sessionsApi"
import { TabsApiError, tabsApi } from "./tabsApi"
import { TerminalsApiError, terminalsApi } from "./terminalsApi"

function stubResponse(status: number, body: string | Error) {
  const text = vi.fn(async () => {
    if (body instanceof Error) throw body
    return body
  })
  const fetchMock = vi.fn(async () => ({
    ok: status >= 200 && status < 300,
    status,
    text,
  })) as unknown as typeof fetch
  vi.stubGlobal("fetch", fetchMock)
  return {
    fetchMock: fetchMock as unknown as ReturnType<typeof vi.fn>,
    text,
  }
}

function lastRequest(fetchMock: ReturnType<typeof vi.fn>) {
  const [path, init] = fetchMock.mock.calls.at(-1) as [string, RequestInit]
  return { path, init }
}

const clients = [
  {
    name: "projectsApi",
    ErrorType: ProjectsApiError,
    requestWithoutBody: () => projectsApi.worktreeCounts(),
    requestWithBody: () => projectsApi.patch("p1", { provider: "codex" }),
    requestWithSerializationFailure: (value: unknown) =>
      projectsApi.patch("p1", value as never),
    mapsSerializationFailure: false,
  },
  {
    name: "sessionsApi",
    ErrorType: SessionsApiError,
    requestWithoutBody: () => sessionsApi.startupLogs("s1"),
    requestWithBody: () => sessionsApi.patch("s1", { provider: "codex" }),
    requestWithSerializationFailure: (value: unknown) =>
      sessionsApi.patch("s1", value as never),
    mapsSerializationFailure: false,
  },
  {
    name: "tabsApi",
    ErrorType: TabsApiError,
    requestWithoutBody: () => tabsApi.remove("s1", "tab1"),
    requestWithBody: () => tabsApi.patch("s1", "tab1", "codex"),
    requestWithSerializationFailure: (value: unknown) =>
      tabsApi.patch("s1", "tab1", value as never),
    mapsSerializationFailure: true,
  },
  {
    name: "terminalsApi",
    ErrorType: TerminalsApiError,
    requestWithoutBody: () => terminalsApi.create("s1"),
    requestWithBody: () => terminalsApi.reorder(["terminal1"]),
    requestWithSerializationFailure: (value: unknown) =>
      terminalsApi.reorder([value as never]),
    mapsSerializationFailure: true,
  },
]

beforeEach(() => {
  setConnectionId("connection-1")
})

afterEach(() => {
  vi.unstubAllGlobals()
  setConnectionId(null)
})

describe.each(clients)("$name JSON transport", (client) => {
  it("uses same-origin credentials and omits JSON headers without a body", async () => {
    const { fetchMock } = stubResponse(204, new Error("body must not be read"))

    await client.requestWithoutBody()

    const { init } = lastRequest(fetchMock)
    expect(init.credentials).toBe("same-origin")
    expect(init.body).toBeUndefined()
    expect(init.headers).toEqual({ "x-connection-id": "connection-1" })
  })

  it("serializes a body and labels it as JSON", async () => {
    const { fetchMock } = stubResponse(204, "")

    await client.requestWithBody()

    const { init } = lastRequest(fetchMock)
    expect(init.headers).toMatchObject({
      "content-type": "application/json",
      "x-connection-id": "connection-1",
    })
    expect(typeof init.body).toBe("string")
    expect(JSON.parse(init.body as string)).toBeTypeOf("object")
  })

  it("preserves the client's serialization-failure behavior", async () => {
    const serializationError = new Error("could not serialize")
    const fetchMock = vi.fn()
    vi.stubGlobal("fetch", fetchMock)
    const value = {
      toJSON() {
        throw serializationError
      },
    }

    const error = await client
      .requestWithSerializationFailure(value)
      .catch((reason: unknown) => reason)

    expect(fetchMock).not.toHaveBeenCalled()
    if (client.mapsSerializationFailure) {
      expect(error).toBeInstanceOf(client.ErrorType)
      expect(error).toMatchObject({
        message: "Could not reach the server.",
        status: 0,
      })
    } else {
      expect(error).toBe(serializationError)
    }
  })

  it("parses a successful JSON response", async () => {
    const { text } = stubResponse(200, JSON.stringify({ current: true }))

    await expect(client.requestWithoutBody()).resolves.toEqual({ current: true })
    expect(text).toHaveBeenCalledOnce()
  })

  it.each([
    ["an empty body", ""],
    ["malformed JSON", "{"],
    ["an unreadable body", new Error("read failed")],
  ])("resolves undefined for %s on a successful response", async (_label, body) => {
    stubResponse(200, body)

    await expect(client.requestWithoutBody()).resolves.toBeUndefined()
  })

  it("does not read a 204 response body", async () => {
    const { text } = stubResponse(204, new Error("body must not be read"))

    await expect(client.requestWithoutBody()).resolves.toBeUndefined()
    expect(text).not.toHaveBeenCalled()
  })

  it("maps a transport failure to the client's typed status-zero error", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new Error("offline")
      }) as unknown as typeof fetch,
    )

    const error = await client.requestWithoutBody().catch((reason: unknown) => reason)
    expect(error).toBeInstanceOf(client.ErrorType)
    expect(error).toMatchObject({
      message: "Could not reach the server.",
      status: 0,
    })
  })

  it("trims the server's error detail", async () => {
    stubResponse(409, "  request refused  \n")

    const error = await client.requestWithoutBody().catch((reason: unknown) => reason)
    expect(error).toBeInstanceOf(client.ErrorType)
    expect(error).toMatchObject({ message: "request refused", status: 409 })
  })

  it.each([
    ["an empty error body", ""],
    ["an unreadable error body", new Error("read failed")],
  ])("uses the status fallback for %s", async (_label, body) => {
    stubResponse(503, body)

    const error = await client.requestWithoutBody().catch((reason: unknown) => reason)
    expect(error).toBeInstanceOf(client.ErrorType)
    expect(error).toMatchObject({ message: "request failed (503)", status: 503 })
  })
})

describe("sessionsApi error bodies", () => {
  it("keeps parsed JSON on a failed response", async () => {
    stubResponse(409, JSON.stringify({ existing_branch: { name: "feature" } }))

    const error = await sessionsApi
      .create({ kind: "new", project_id: "p1" })
      .catch((reason: unknown) => reason)

    expect(error).toBeInstanceOf(SessionsApiError)
    expect(error).toMatchObject({
      body: { existing_branch: { name: "feature" } },
    })
  })

  it("uses null when a failed response is not JSON", async () => {
    stubResponse(409, "plain text")

    const error = await sessionsApi
      .create({ kind: "new", project_id: "p1" })
      .catch((reason: unknown) => reason)

    expect(error).toBeInstanceOf(SessionsApiError)
    expect(error).toMatchObject({ body: null })
  })
})
