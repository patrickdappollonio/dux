// A take-over this page pressed that the server has not answered yet, by row.
//
// A press on the card lowers it and enables the row's controls at once, while
// on the server the claim rides the reconnecting terminal socket's first
// resize. A change to that row sent in between would be refused although this
// page has just taken over, so `apiFetch` holds it here until the claim has
// landed, and does not send it at all when the claim failed or is taking too
// long. The pane that pressed opens the flight and settles it on the server's
// word (`components/terminal/ownership.ts`).

import { useSyncExternalStore } from "react"

/// How long a change waits for a take-over before it is given up on.
export const TAKEOVER_WAIT_MS = 4000

type Outcome = "landed" | "failed"

const flights = new Map<string, Promise<Outcome>>()
const listeners = new Set<() => void>()

function changed(): void {
  for (const listener of [...listeners]) listener()
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

/// Open a flight for `row` (a `paneCover` row key) and return what settles it:
/// true once the server named this page the row's driver, false when the
/// claim was lost. Settling twice, or after a later press replaced the flight,
/// does nothing more.
export function beginTakeoverFlight(row: string): (landed: boolean) => void {
  let settle: (outcome: Outcome) => void = () => {}
  const flight = new Promise<Outcome>((resolve) => {
    settle = resolve
  })
  flights.set(row, flight)
  changed()
  return (landed) => {
    if (flights.get(row) === flight) {
      flights.delete(row)
      changed()
    }
    settle(landed ? "landed" : "failed")
  }
}

/// The row keys a request to `path` may change, read from the address alone:
/// an agent's routes and a terminal's. Empty for a request that names no row.
/// A dropped file names its pane's pty and a reorder of the terminals names
/// them in its body, neither of which says which rows change, so each asks by
/// row itself (`takeoverRefusalForRows`).
function rowsOfRequest(path: string): string[] {
  const [route] = path.split("?", 1)
  const segments = (route ?? "").split("/").map((segment) => {
    try {
      return decodeURIComponent(segment)
    } catch {
      return segment
    }
  })
  const after = (name: string): string | undefined => {
    const at = segments.indexOf(name)
    return at === -1 ? undefined : segments[at + 1]
  }
  const session = after("sessions")
  if (session !== undefined && session !== "") return [`agent:${session}`]
  const terminal = after("terminals")
  if (terminal === "reorder") return []
  if (terminal !== undefined && terminal !== "") return [`terminal:${terminal}`]
  return []
}

/// Is a take-over this page pressed on `row` still waiting for the server?
export function takeoverInFlight(row: string): boolean {
  return flights.has(row)
}

/// The same answer, re-rendering when it changes.
export function useTakeoverInFlight(row: string): boolean {
  return useSyncExternalStore(subscribe, () => flights.has(row))
}

/// Wait out a take-over in flight on a row `path` would change. Null, at once,
/// when there is none: a request must then leave in the same turn it was made
/// in, as it always has. Otherwise a promise of null once the claim has
/// landed, or of the sentence to refuse the request with, unsent.
export function takeoverRefusalFor(path: string): Promise<string | null> | null {
  if (flights.size === 0) return null
  return refusalForRows(rowsOfRequest(path))
}

/// The same wait for a request known to change `row`.
export function takeoverRefusalForRow(row: string): Promise<string | null> | null {
  return refusalForRows([row])
}

/// The same wait for a request known to change `rows`.
export function takeoverRefusalForRows(rows: readonly string[]): Promise<string | null> | null {
  return refusalForRows(rows)
}

function refusalForRows(rows: readonly string[]): Promise<string | null> | null {
  const waits = rows
    .map((row) => flights.get(row))
    .filter((found) => found !== undefined)
    .map(settled)
  if (waits.length === 0) return null
  // The first refusal of any of them, once every one has settled.
  return Promise.all(waits).then((said) => said.find((s) => s !== null) ?? null)
}

async function settled(flight: Promise<Outcome>): Promise<string | null> {
  let timer: ReturnType<typeof setTimeout> | undefined
  const late = new Promise<"late">((resolve) => {
    timer = setTimeout(() => resolve("late"), TAKEOVER_WAIT_MS)
  })
  const outcome = await Promise.race([flight, late])
  clearTimeout(timer)
  switch (outcome) {
    case "landed":
      return null
    case "failed":
      return "This device's take-over of it did not go through, so this was not sent and nothing was changed. Take it over again, then retry."
    case "late":
      return "This device is still taking it over, so this was not sent and nothing was changed. Try again in a moment."
  }
}
