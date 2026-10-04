import { afterEach, describe, expect, it, vi } from "vitest"

import { apiUrl, wsUrl } from "./apiBase"

afterEach(() => {
  vi.unstubAllGlobals()
})

describe("apiUrl", () => {
  it("keeps an API path relative to the page that served it", () => {
    expect(apiUrl("/api/v1/workspace")).toBe("/api/v1/workspace")
  })

  it("keeps the query string as it was given", () => {
    expect(apiUrl("/api/v1/browse?path=%2Ftmp")).toBe("/api/v1/browse?path=%2Ftmp")
  })
})

describe("wsUrl", () => {
  it("uses ws on a plain HTTP page and the page's own host and port", () => {
    vi.stubGlobal("location", { protocol: "http:", host: "127.0.0.1:8080" })
    expect(wsUrl("/ws/events")).toBe("ws://127.0.0.1:8080/ws/events")
  })

  it("uses wss on an HTTPS page, so the socket is not blocked as mixed content", () => {
    vi.stubGlobal("location", { protocol: "https:", host: "dux.example.test" })
    expect(wsUrl("/ws/sessions/a/pty")).toBe("wss://dux.example.test/ws/sessions/a/pty")
  })

  it("reads the location at call time, not at import", () => {
    vi.stubGlobal("location", { protocol: "http:", host: "a:1" })
    expect(wsUrl("/ws/events")).toBe("ws://a:1/ws/events")
    vi.stubGlobal("location", { protocol: "https:", host: "b:2" })
    expect(wsUrl("/ws/events")).toBe("wss://b:2/ws/events")
  })
})
