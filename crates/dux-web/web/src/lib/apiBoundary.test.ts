import { readFileSync, readdirSync } from "node:fs"
import { join, relative, sep } from "node:path"
import { fileURLToPath } from "node:url"

import { describe, expect, it } from "vitest"

// The boundary that keeps every request and socket behind the auth gate and
// the one URL helper.
//
// A module calling `fetch` directly skips `apiFetch`: its 401 never signs the
// page out, it keeps sending while signed out, and an answer from an ended
// session lands as if nothing happened. A module building a socket URL from
// `location.host` skips `apiBase.ts`, the one place a UI served from elsewhere
// has to change. Both lists are exact sets, the idiom `notifyBoundary.test.ts`
// explains: a new entry has to be added here and defended in review.
//
// It greps text with comments stripped, so it catches the obvious call typed
// in a hurry, not a deliberate evasion.

const SRC = join(fileURLToPath(new URL(".", import.meta.url)), "..")

/// The only production modules that call `fetch`: the door itself, and the auth
/// routes, which must work while signed out and read a 401 as a wrong password.
const FETCH_CALLERS = ["lib/apiFetch.ts", "lib/authApi.ts"]

/// The only production module that reads the page's host to build a URL.
const HOST_READERS = ["lib/apiBase.ts"]

/// The only production module that constructs a WebSocket.
const SOCKET_CONSTRUCTORS = ["lib/reconnectingSocket.ts"]

function sourceFiles(dir: string): string[] {
  const out: string[] = []
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name)
    if (entry.isDirectory()) {
      out.push(...sourceFiles(full))
      continue
    }
    if (/\.tsx?$/.test(entry.name) && !/\.test\.tsx?$/.test(entry.name)) out.push(full)
  }
  return out
}

// A line comment only where `//` starts a line or follows whitespace, so the
// `//` inside a URL template (`${scheme}//${host}`) is not read as one.
function stripComments(source: string): string {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/(^|\s)\/\/[^\n]*/g, "$1")
}

function matching(pattern: RegExp): string[] {
  return sourceFiles(SRC)
    .filter((file) => pattern.test(stripComments(readFileSync(file, "utf8"))))
    .map((file) => relative(SRC, file).split(sep).join("/"))
    .sort()
}

describe("the request boundary", () => {
  it("lets exactly the door and the auth routes call fetch", () => {
    // `fetch(` not preceded by a word character or a dot, so `apiFetch(`,
    // `refetch(` and `x.fetch(` are not calls of the global.
    expect(matching(/(?<![\w.$])fetch\s*\(/)).toEqual(FETCH_CALLERS)
  })

  it("lets exactly the URL helper read the page's host", () => {
    expect(matching(/location\.host\b/)).toEqual(HOST_READERS)
  })

  it("lets exactly the shared socket base open a WebSocket", () => {
    expect(matching(/new\s+WebSocket\s*\(/)).toEqual(SOCKET_CONSTRUCTORS)
  })

  it("actually reads files, so an empty scan can never pass by accident", () => {
    expect(sourceFiles(SRC).length).toBeGreaterThan(100)
  })
})
