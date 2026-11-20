import { coverOwnsThePane, type AttachCover } from "./attachCover"
import { createKeyedRegistry } from "./keyedRegistry"

// Which cover is over one pane, keyed by pty id, for the chrome that sits
// outside the pane. Only the pane knows (it is the one that resolves the
// cover), and the shells' top chrome has to know, so the pane publishes and the
// shell reads.

/// Boxed so each registration has an identity of its own: the registry tells
/// registrations apart by identity, and two panes publishing the same verdict
/// must still be told apart when the outgoing one retires.
interface CoverEntry {
  kind: AttachCover["kind"]
}

const covers = createKeyedRegistry<CoverEntry>()

/**
 * Publish this pane's cover verdict and return its retirement. The retirement
 * clears the entry only while it is still this pane's, so a replacement pane
 * survives the outgoing pane's late cleanup.
 */
export function registerPaneCover(
  ptyId: string,
  kind: AttachCover["kind"],
): () => void {
  return covers.register(ptyId, { kind })
}

/**
 * Does a full-pane cover own this pty's pane? An id nobody has published for
 * reads as uncovered, which is what an unmounted or dormant pane is.
 */
export function usePaneCoverOwned(ptyId: string | null): boolean {
  covers.useVersion()
  return paneCoverOwnedFor(ptyId)
}

/** Does a full-pane cover own any of these ptys' panes? */
export function useAnyPaneCoverOwned(ptyIds: readonly string[]): boolean {
  covers.useVersion()
  return ptyIds.some((id) => paneCoverOwnedFor(id))
}

/**
 * Is the take-over card up over this pty's pane? It is the cover of a row this
 * device does not drive, and ownership is row-wide, so the pane typing goes to
 * answers for the whole row. An id nobody has published for reads as no.
 */
export function useTakeoverCardUp(ptyId: string | null): boolean {
  covers.useVersion()
  return ptyId !== null && covers.read(ptyId)?.kind === "card"
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
  return entry !== undefined && coverOwnsThePane(entry)
}

/** Test-only: forget every registration between cases. */
export function resetPaneCovers(): void {
  covers.reset()
}
