// Which folders are expanded in the Changes pane, per agent, for as long as
// the page lives: not persisted, and not in the URL. Held outside React so it
// survives the pane unmounting (switching agents, a phone moving between
// screens) and outside the app store so an expansion re-renders the pane and
// nothing else.
//
// This module also owns the requests for folders' contents, one in flight per
// folder at most: a newer request (a quiet refresh after the listing moved)
// aborts the older one, and collapsing a folder aborts what it was waiting for.

import { useSyncExternalStore } from "react"

import { ChangesFetchAborted, fetchFolderChildren } from "./changesApi"
import {
  NO_EXPANSIONS,
  collapseFolder,
  expandFolder,
  failFolder,
  folderKey,
  reconcileExpansions,
  retryFolder,
  settleFolderAndReconcile,
  type Expansions,
} from "./changesTree"
import type { ChangesSection } from "./changesWindow"
import type { ChangedFileView } from "./types"

const bySession = new Map<string, Expansions>()
const inflight = new Map<string, AbortController>()
const listeners = new Set<() => void>()

const requestKey = (sessionId: string, key: string) => `${sessionId}|${key}`

// The expanded folders of one agent.
export function expansionsFor(sessionId: string): Expansions {
  return bySession.get(sessionId) ?? NO_EXPANSIONS
}

function publish(sessionId: string, next: Expansions): void {
  if (next === expansionsFor(sessionId)) return
  if (next.size === 0) bySession.delete(sessionId)
  else bySession.set(sessionId, next)
  for (const listener of listeners) listener()
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

// A server render (the docs site pre-renders the Changes pane) has no page
// lifetime to remember expansions over, so it sees nothing expanded. The
// browser hydrates with the same answer, since the store starts empty.
function serverSnapshot(): Expansions {
  return NO_EXPANSIONS
}

// The expanded folders of one agent, re-rendering its reader when they move.
export function useExpansions(sessionId: string): Expansions {
  return useSyncExternalStore(
    subscribe,
    () => expansionsFor(sessionId),
    serverSnapshot,
  )
}

function abortRequest(sessionId: string, key: string): void {
  const id = requestKey(sessionId, key)
  inflight.get(id)?.abort()
  inflight.delete(id)
}

// Abort every request for `section:path` and anything under it.
function abortSubtree(sessionId: string, section: ChangesSection, path: string): void {
  const own = requestKey(sessionId, folderKey(section, path))
  for (const [id, controller] of inflight) {
    if (id === own || id.startsWith(`${own}/`)) {
      controller.abort()
      inflight.delete(id)
    }
  }
}

// Ask for one folder's contents, superseding any request for it in flight.
function request(sessionId: string, section: ChangesSection, path: string): void {
  const key = folderKey(section, path)
  abortRequest(sessionId, key)
  const controller = new AbortController()
  const id = requestKey(sessionId, key)
  inflight.set(id, controller)
  fetchFolderChildren(sessionId, path, section, controller.signal).then(
    (answer) => {
      if (inflight.get(id) !== controller) return
      inflight.delete(id)
      // The folders expanded inside it are checked against these children:
      // one that is gone is forgotten, one that moved is asked for again.
      const current = expansionsFor(sessionId)
      const { next, refetch } = settleFolderAndReconcile(
        current,
        section,
        path,
        answer.children,
      )
      for (const key of current.keys()) {
        if (!next.has(key)) abortRequest(sessionId, key)
      }
      publish(sessionId, next)
      for (const moved of refetch) request(sessionId, moved.section, moved.path)
    },
    (error: unknown) => {
      if (error instanceof ChangesFetchAborted || inflight.get(id) !== controller) return
      inflight.delete(id)
      const message = error instanceof Error ? error.message : "Could not list this folder."
      publish(sessionId, failFolder(expansionsFor(sessionId), section, path, message))
    },
  )
}

// Expand a folded folder row, or collapse it when it is expanded.
export function toggleFolder(
  sessionId: string,
  section: ChangesSection,
  row: ChangedFileView,
): void {
  const current = expansionsFor(sessionId)
  if (current.has(folderKey(section, row.path))) {
    abortSubtree(sessionId, section, row.path)
    publish(sessionId, collapseFolder(current, section, row.path))
    return
  }
  publish(sessionId, expandFolder(current, section, row))
  request(sessionId, section, row.path)
}

// Ask again for a folder whose contents could not be listed.
export function retryFolderChildren(
  sessionId: string,
  section: ChangesSection,
  path: string,
): void {
  publish(sessionId, retryFolder(expansionsFor(sessionId), section, path))
  request(sessionId, section, path)
}

// Bring one agent's expanded folders in line with a new listing: forget the
// folders it no longer shows, and ask again, quietly, for those whose rows
// moved.
export function reconcileFolders(
  sessionId: string,
  staged: readonly ChangedFileView[],
  unstaged: readonly ChangedFileView[],
): void {
  const current = expansionsFor(sessionId)
  if (current.size === 0) return
  const { next, refetch } = reconcileExpansions(current, staged, unstaged)
  for (const key of current.keys()) {
    if (!next.has(key)) abortRequest(sessionId, key)
  }
  publish(sessionId, next)
  for (const { section, path } of refetch) request(sessionId, section, path)
}

// Forget everything, for tests.
export function resetExpansionsForTests(): void {
  for (const controller of inflight.values()) controller.abort()
  inflight.clear()
  bySession.clear()
  for (const listener of listeners) listener()
}
