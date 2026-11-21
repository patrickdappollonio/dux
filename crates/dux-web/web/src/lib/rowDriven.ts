// The server's refusal of a change to a row another device drives, as the
// browser's request helpers see it.

/// The status of that refusal. `apiFetch` answers with the same one for a
/// change it holds back while this page's own take-over is in flight.
export const ROW_DRIVEN_STATUS = 423

/// Whether a request failed because another device drives the row it would
/// change. Every request of one batch to that row would get the same answer,
/// so a batch stops at the first.
export function isRowDriven(error: unknown): error is Error & { status: number } {
  return error instanceof Error && (error as { status?: unknown }).status === ROW_DRIVEN_STATUS
}
