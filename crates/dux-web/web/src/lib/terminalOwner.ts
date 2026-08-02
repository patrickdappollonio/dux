// Terminal ownership, defined ONCE and switched on exhaustively.
//
// A terminal is owned by exactly one owner and ownership never changes after
// spawn. Terminals used to reach the client only by being nested inside the
// session or the project that owns them, with no owner on the terminal at all,
// so every consumer inferred ownership from which collection it had walked, or
// from a two-way `owner.kind === "session" ? ... : ...` conditional. Both are
// silent: add a third kind of owner and the conditional keeps compiling and
// treats it as a project.
//
// So the owner is now a tagged value the server sends (`TerminalOwnerWire`, the
// mirror of Rust's `dux_core::viewmodel::TerminalOwnerView`), and every decision
// it drives goes through a `switch` whose last statement is `assertNever`. A new
// variant is then a COMPILE error at every site that has to answer for it.

import { assertNever } from "@/lib/assertNever"

// The serialized owner, exactly as it arrives on `TerminalView.owner`. Field
// names are the server's (snake_case); `TerminalOwnerRef` below is the
// client-side spelling used by the store and the URL.
export type TerminalOwnerWire =
  | { kind: "session"; session_id: string }
  | { kind: "project"; project_id: string }
  // Owned by nothing: a standalone terminal, opened in the user's home
  // directory. There is no owner id to carry, so it carries what its row names
  // it by instead: the server-side directory it opened in, already written with
  // the home directory collapsed to `~`. The server shortens it because the
  // browser is not necessarily on the same machine and has no `~` of the
  // server's to collapse against.
  | { kind: "standalone"; cwd_label: string }

// The client-side owner reference: the same union, in the app's own spelling.
// It is what the selected target carries, what the deep-link parser produces,
// and what the REST/websocket URL builders take.
export type TerminalOwnerRef =
  | { kind: "session"; sessionId: string }
  | { kind: "project"; projectId: string }
  // No id, because there is no owner. Every standalone terminal shares this one
  // reference value, which is exactly right: the terminal id is what tells two
  // of them apart, and the address they live at (`#/terminal/<id>`) carries no
  // owner segment for the same reason.
  | { kind: "standalone" }

// Wire → client. The one ingestion point for ownership, so nothing downstream
// reads the server's field names.
export function ownerRefFromWire(owner: TerminalOwnerWire): TerminalOwnerRef {
  switch (owner.kind) {
    case "session":
      return { kind: "session", sessionId: owner.session_id }
    case "project":
      return { kind: "project", projectId: owner.project_id }
    case "standalone":
      return { kind: "standalone" }
    default:
      return assertNever(owner)
  }
}

// A handler per owner variant, mapped over the union's `kind`. This is the
// second half of the guarantee, and it exists because a `switch` inside a HELPER
// only protects the helper: `ownerSessionId` below is exhaustive, yet a caller
// that reduces an owner to that nullable id keeps compiling the day a new
// variant starts answering null, and quietly does the wrong thing with it. So a
// consumer whose BEHAVIOUR depends on the owner takes one of these instead. The
// object literal it writes is missing a key the moment a variant is added, which
// is a compile error at the CONSUMER, where the decision actually lives.
//
// Use `matchOwner`/`matchWireOwner` wherever the owner selects what is rendered,
// where a row is emitted, or what something is called. Use `ownerSessionId` and
// the two-bucket grouping only where "is it a session or not" really is the
// whole question, and say so at the call site.
export type OwnerMatch<T> = {
  [K in TerminalOwnerRef["kind"]]: (
    owner: Extract<TerminalOwnerRef, { kind: K }>,
  ) => T
}

export function matchOwner<T>(owner: TerminalOwnerRef, on: OwnerMatch<T>): T {
  switch (owner.kind) {
    case "session":
      return on.session(owner)
    case "project":
      return on.project(owner)
    case "standalone":
      return on.standalone(owner)
    default:
      return assertNever(owner)
  }
}

// The wire spelling of the same matcher, for consumers holding a
// `TerminalView.owner` straight off the spine that never need the client
// spelling.
export type WireOwnerMatch<T> = {
  [K in TerminalOwnerWire["kind"]]: (
    owner: Extract<TerminalOwnerWire, { kind: K }>,
  ) => T
}

export function matchWireOwner<T>(
  owner: TerminalOwnerWire,
  on: WireOwnerMatch<T>,
): T {
  switch (owner.kind) {
    case "session":
      return on.session(owner)
    case "project":
      return on.project(owner)
    case "standalone":
      return on.standalone(owner)
    default:
      return assertNever(owner)
  }
}

// The owning SESSION's id, or null when this terminal belongs to something that
// is not a session. Session-scoped UI (the changes pane, the agent breadcrumb,
// the session PTY route) hangs off this, and every one of those surfaces already
// tolerates null, which is what makes null the right answer for a non-session
// owner rather than a special case at each call site.
//
// LOSSY ON PURPOSE. It collapses every non-session owner into one answer, so it
// can only be used where "is it a session or not" is the ENTIRE decision, and
// each such use must say so. Anything that has to SAY something about the owner
// (name it, place its row, choose its screen) uses `matchOwner` above instead.
export function ownerSessionId(owner: TerminalOwnerRef): string | null {
  switch (owner.kind) {
    case "session":
      return owner.sessionId
    case "project":
    case "standalone":
      return null
    default:
      return assertNever(owner)
  }
}

// The owning PROJECT's id, or null when this terminal belongs to something that
// is not a project. Lossy on purpose, on the same terms as `ownerSessionId`.
export function ownerProjectId(owner: TerminalOwnerRef): string | null {
  switch (owner.kind) {
    case "session":
      return null
    case "standalone":
      return null
    case "project":
      return owner.projectId
    default:
      return assertNever(owner)
  }
}

// Whether a wire owner and a client owner reference name the same owner. Used
// to select a terminal's siblings and to test membership without unpacking the
// two spellings by hand.
export function sameOwner(
  wire: TerminalOwnerWire,
  ref: TerminalOwnerRef,
): boolean {
  switch (wire.kind) {
    case "session":
      return ref.kind === "session" && ref.sessionId === wire.session_id
    case "project":
      return ref.kind === "project" && ref.projectId === wire.project_id
    // Every standalone terminal has the same owner, so this is a kind test and
    // nothing more. Two standalone terminals ARE each other's siblings, which
    // is what makes `terminalTitle` disambiguate two of them running `vim`.
    case "standalone":
      return ref.kind === "standalone"
    default:
      return assertNever(wire)
  }
}

// Whether two wire owners name the same owner. Backs the sibling grouping in
// `assembleFlatTerminals` (a terminal's siblings are the terminals sharing its
// owner), which used to be free because siblings were literally the array a
// terminal was nested in.
export function sameWireOwner(
  a: TerminalOwnerWire,
  b: TerminalOwnerWire,
): boolean {
  switch (a.kind) {
    case "session":
      return b.kind === "session" && a.session_id === b.session_id
    case "project":
      return b.kind === "project" && a.project_id === b.project_id
    case "standalone":
      return b.kind === "standalone"
    default:
      return assertNever(a)
  }
}

// A stable string key for an owner, for grouping terminals by owner in a Map.
// The `kind` prefix is what keeps a session and a project that happen to share
// an id from colliding into one group.
export function ownerKey(owner: TerminalOwnerWire): string {
  switch (owner.kind) {
    case "session":
      return `session:${owner.session_id}`
    case "project":
      return `project:${owner.project_id}`
    // One group for every standalone terminal, which is correct: they all share
    // the one owner. The `standalone` prefix cannot collide with an id-bearing
    // key, and the absence of an id after it is the point.
    case "standalone":
      return "standalone"
    default:
      return assertNever(owner)
  }
}
