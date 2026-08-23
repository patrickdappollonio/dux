// What the code editor is rooted at.
//
// The root is a tagged value (agents and terminals draw ids from different
// counters), and the API URL, tab-list key, draft-cache key, React key and
// address all ask this module rather than reading a raw id.
//
// The root is pinned at spawn and is never the shell's live working directory.
// A file dropped on a terminal PANE follows the shell, because that is where
// the user is typing; an editor root backs a tree, a set of buffers, their
// drafts and a bookmarkable URL, and all four would be invalidated the moment
// somebody typed `cd`.
//
// Every question about a root is answered by a switch that ends in
// `assertNever`, so a third kind of root cannot be added without answering all
// of them.

import { assertNever } from "./assertNever"
import { matchOwner, type TerminalOwnerRef } from "./terminalOwner"

// The terminal shape is the one `SelectedTarget` already uses, reused rather
// than restated: a terminal is identified by its id AND its owner everywhere
// else in the app, and a second nearly-identical union would be a thing to keep
// in step by hand.
export interface TerminalTarget {
  kind: "terminal"
  terminalId: string
  owner: TerminalOwnerRef
}

export type EditorRoot = { kind: "agent"; sessionId: string } | TerminalTarget

export function agentRoot(sessionId: string): EditorRoot {
  return { kind: "agent", sessionId }
}

// The root an editor opened from `target` should use.
//
// This is the one place the session-owned terminal case is decided, and it is
// the reason the decision is a function rather than a cast. A terminal spawned
// in an agent's worktree already has an editor, the agent's, with the full git
// surface, diff mode and changed-files freshness. So its rows and its existing
// `#/agent/<sid>/terminal/<tid>/editor` address keep meaning the agent's
// worktree, and no terminal root is ever built for one.
export function editorRootForTarget(target: EditorRoot): EditorRoot {
  if (target.kind === "agent") return agentRoot(target.sessionId)
  return matchOwner<EditorRoot>(target.owner, {
    session: (owner) => agentRoot(owner.sessionId),
    project: () => target,
    standalone: () => target,
  })
}

// The string key for `editorTabs`, the draft cache, and React keys. Namespaced
// because agent ids and terminal ids are minted by different counters and
// nothing stops them colliding.
export function rootKey(root: EditorRoot): string {
  switch (root.kind) {
    case "agent":
      return `agent:${root.sessionId}`
    case "terminal":
      return `terminal:${root.terminalId}`
    default:
      return assertNever(root)
  }
}

// The API prefix every editor file route hangs off. Each root is served from
// the namespace that owns it, and the server refuses any other one.
export function rootApiBase(root: EditorRoot): string {
  switch (root.kind) {
    case "agent":
      return sessionApiBase(root.sessionId)
    case "terminal":
      return matchOwner(root.owner, {
        // Not reachable through `editorRootForTarget`, which sends a
        // session-owned terminal to its agent root before anything asks for a
        // URL. It is still the right answer: the terminal shares that agent's
        // worktree, and the agent's address is where the server serves it.
        session: (owner) => sessionApiBase(owner.sessionId),
        project: (owner) =>
          `/api/v1/projects/${encodeURIComponent(owner.projectId)}/terminals/${encodeURIComponent(root.terminalId)}`,
        standalone: () => `/api/v1/terminals/${encodeURIComponent(root.terminalId)}`,
      })
    default:
      return assertNever(root)
  }
}

function sessionApiBase(sessionId: string): string {
  return `/api/v1/sessions/${encodeURIComponent(sessionId)}`
}

// The pty id a file dropped on this editor's TREE is uploaded through. The
// upload route is keyed by pty rather than by editor root, and resolves an
// agent id and a terminal id alike.
export function rootPtyId(root: EditorRoot): string {
  switch (root.kind) {
    case "agent":
      return root.sessionId
    case "terminal":
      return root.terminalId
    default:
      return assertNever(root)
  }
}

// The session whose changed files, diff mode and git actions belong to this
// root, or null when there is none. A terminal root answers null and the
// absence is the design: no changes pane, no diff mode, no broadcast.
export function rootSessionId(root: EditorRoot): string | null {
  switch (root.kind) {
    case "agent":
      return root.sessionId
    case "terminal":
      return null
    default:
      return assertNever(root)
  }
}

// Does this root have a diff to show? Only an agent's does: diff mode is HEAD
// against the working copy, and a terminal root is a plain directory that may
// not be in a repository at all. The server registers no diff route for one, so
// the affordance is absent rather than disabled, and the mode is refused rather
// than merely unoffered (an address can still ask for it).
export function rootHasDiff(root: EditorRoot): boolean {
  return rootSessionId(root) !== null
}

export function sameRoot(a: EditorRoot | null, b: EditorRoot | null): boolean {
  if (a === null || b === null) return a === b
  return rootKey(a) === rootKey(b)
}
