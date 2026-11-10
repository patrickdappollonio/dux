import { createKeyedRegistry } from "./keyedRegistry"

// Whether a full-pane cover owns one pane, keyed by pty id, for the chrome that
// sits outside the pane. Only the pane knows (it is the one that resolves the
// cover), and the shells' top chrome has to know, so the pane publishes and the
// shell reads.

/// Boxed so each registration has an identity of its own: the registry tells
/// registrations apart by identity, and two panes publishing the same verdict
/// must still be told apart when the outgoing one retires.
interface CoverEntry {
  ownsPane: boolean
}

const covers = createKeyedRegistry<CoverEntry>()

/**
 * Publish this pane's cover verdict and return its retirement. The retirement
 * clears the entry only while it is still this pane's, so a replacement pane
 * survives the outgoing pane's late cleanup.
 */
export function registerPaneCover(ptyId: string, ownsPane: boolean): () => void {
  return covers.register(ptyId, { ownsPane })
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
  return ptyId === null ? false : (covers.read(ptyId)?.ownsPane ?? false)
}

/** Test-only: forget every registration between cases. */
export function resetPaneCovers(): void {
  covers.reset()
}
