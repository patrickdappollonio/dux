import { coverOwnsThePane, type AttachCover } from "./attachCover"
import { createKeyedRegistry } from "./keyedRegistry"

// Which cover is over one pane, keyed by pty id, for the chrome that sits
// outside the pane. Only the pane knows (it is the one that resolves the
// cover), and the shells' top chrome has to know, so the pane publishes and the
// shell reads.
//
// A device drives or watches a whole row of the agents list, so every pane also
// names its row, and the row's answer is the one the chrome reads: the row is
// watched while ANY of its panes shows the take-over card. Its parts can
// disagree for a moment (a pane whose socket just failed, or one that remounted
// and has not heard the server yet, has no card of its own), and the row then
// stays covered rather than showing what the other part is watching.

/// Boxed so each registration has an identity of its own: the registry tells
/// registrations apart by identity, and two panes publishing the same verdict
/// must still be told apart when the outgoing one retires.
interface CoverEntry {
  kind: AttachCover["kind"]
  row: string
  /// Somebody drives the pty, as far as this pane knows. Read only under the
  /// card, where false is the card's "Running in the background".
  ownerPresent: boolean
}

const covers = createKeyedRegistry<CoverEntry>()

/// The take-over pressed on a row's card, by the pty whose card was pressed,
/// keyed by row. Until the server's answer reaches the row's other panes they
/// still show their cards, and the device that pressed is the one driving.
interface RowClaim {
  ptyId: string
}

const claims = createKeyedRegistry<RowClaim>()

/** The row key of an agent: its tabs and its own terminals. */
export function agentRowKey(sessionId: string): string {
  return `agent:${sessionId}`
}

/** The row key of the part a pane streams. */
export function rowKeyOfPart(
  part:
    | { kind: "agent"; sessionId: string }
    | { kind: "terminal"; id: string; owner: { kind: string; sessionId?: string } },
): string {
  if (part.kind === "agent") return agentRowKey(part.sessionId)
  return part.owner.kind === "session" && part.owner.sessionId !== undefined
    ? agentRowKey(part.owner.sessionId)
    : `terminal:${part.id}`
}

/**
 * Publish this pane's cover verdict and return its retirement. The retirement
 * clears the entry only while it is still this pane's, so a replacement pane
 * survives the outgoing pane's late cleanup. A pane that names no row is a row
 * of its own.
 */
export function registerPaneCover(
  ptyId: string,
  kind: AttachCover["kind"],
  opts: { row?: string; ownerPresent?: boolean } = {},
): () => void {
  return covers.register(ptyId, {
    kind,
    row: opts.row ?? `pty:${ptyId}`,
    ownerPresent: opts.ownerPresent ?? true,
  })
}

/**
 * Record that this page pressed Take over on a row's card, through the pane
 * `ptyId`, and return its retirement. While that pane is not itself back under
 * a card or a box, the row reads as driven.
 */
export function registerRowClaim(row: string, ptyId: string): () => void {
  return claims.register(row, { ptyId })
}

/** The cover verdict one pane published, or undefined. */
export function paneCoverKindFor(ptyId: string): AttachCover["kind"] | undefined {
  return covers.read(ptyId)?.kind
}

function claimHolds(row: string): boolean {
  const claim = claims.read(row)
  if (claim === undefined) return false
  const presser = covers.read(claim.ptyId)
  return (
    presser !== undefined &&
    presser.row === row &&
    (presser.kind === "none" || presser.kind === "spinner")
  )
}

function rowCards(row: string): CoverEntry[] {
  return Array.from(covers.values()).filter(
    (entry) => entry.row === row && entry.kind === "card",
  )
}

/**
 * Is a claim pressed on this row finished with: answered by the server (no
 * pane of the row shows the card any more), refused or lost (the pane pressed
 * is back under a card or a box), or gone with its pane?
 */
export function rowClaimSpent(row: string): boolean {
  const claim = claims.read(row)
  if (claim === undefined) return false
  const presser = covers.read(claim.ptyId)
  return (
    presser === undefined ||
    presser.kind === "card" ||
    presser.kind === "box" ||
    rowCards(row).length === 0
  )
}

/** Does this device watch the row rather than drive it? */
export function rowWatchedFor(row: string | null): boolean {
  if (row === null || claimHolds(row)) return false
  return rowCards(row).length > 0
}

/** Is the row watched because another device drives it, rather than because
 * nobody does and this device has not claimed it? */
export function rowDrivenElsewhereFor(row: string | null): boolean {
  if (row === null || claimHolds(row)) return false
  return rowCards(row).some((entry) => entry.ownerPresent)
}

export type RowWatch = {
  /** The take-over card is up for the row. */
  watched: boolean
  /** ...and another device drives it. */
  elsewhere: boolean
}

/** The row's answer, re-read on every change to any pane of any row. */
export function useRowWatch(row: string | null): RowWatch {
  covers.useVersion()
  claims.useVersion()
  return { watched: rowWatchedFor(row), elsewhere: rowDrivenElsewhereFor(row) }
}

/** The answer for an agent's row, or a quiet one with no agent. */
export function useAgentRowWatch(sessionId: string | null | undefined): RowWatch {
  return useRowWatch(sessionId == null ? null : agentRowKey(sessionId))
}

/** Hear every change of any pane's verdict or any claim, outside React. */
export function subscribePaneCovers(listener: () => void): () => void {
  const fromCovers = covers.subscribe(listener)
  const fromClaims = claims.subscribe(listener)
  return () => {
    fromCovers()
    fromClaims()
  }
}

/**
 * Does a full-pane cover own this pty's pane? An id nobody has published for
 * reads as uncovered, which is what an unmounted or dormant pane is. A pane of
 * a watched row is covered whatever it shows itself, because the row's card is
 * over it.
 */
export function usePaneCoverOwned(ptyId: string | null): boolean {
  covers.useVersion()
  claims.useVersion()
  return paneCoverOwnedFor(ptyId)
}

/** Does a full-pane cover own any of these ptys' panes? */
export function useAnyPaneCoverOwned(ptyIds: readonly string[]): boolean {
  covers.useVersion()
  claims.useVersion()
  return ptyIds.some((id) => paneCoverOwnedFor(id))
}

/**
 * Has the pane behind this id published a verdict at all? "Uncovered" and "no
 * pane has answered yet" read the same through `usePaneCoverOwned`, and chrome
 * that animates on a change must tell them apart: a pane mounting late turns
 * its first publish into a change the user never saw.
 */
export function usePaneCoverKnown(ptyId: string | null): boolean {
  covers.useVersion()
  return ptyId !== null && covers.read(ptyId) !== undefined
}

/// The same verdict without the subscription, for a caller that is not a
/// component and so needs no re-render.
export function paneCoverOwnedFor(ptyId: string | null): boolean {
  const entry = ptyId === null ? undefined : covers.read(ptyId)
  return entry !== undefined && (coverOwnsThePane(entry) || rowWatchedFor(entry.row))
}

/** Test-only: forget every registration between cases. */
export function resetPaneCovers(): void {
  covers.reset()
  claims.reset()
}
