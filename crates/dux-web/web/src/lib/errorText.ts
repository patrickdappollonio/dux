// An error caught by a boundary and where it happened, as the text an error
// screen shows and the details a person copies into a report.
export type Caught = { error: unknown; componentStack: string }

export function messageOf(error: unknown): string {
  if (error instanceof Error) return error.message || error.name
  return String(error)
}

export function detailsOf({ error, componentStack }: Caught): string {
  const stack = error instanceof Error && error.stack ? error.stack : messageOf(error)
  return `${stack}\n\nComponent stack:${componentStack}`
}
