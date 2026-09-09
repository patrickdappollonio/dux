// The exhaustiveness helper: call it in the fall-through of a switch over a
// discriminated union, where the value narrows to `never` only once every variant is
// handled, so a new variant with no case fails `tsc` at the call site rather than at
// the moment a user hits it.
//
// It still throws, because a value reaching here means the data did not match the
// type, and failing loudly beats carrying on with a value nothing understands.
export function assertNever(value: never): never {
  throw new Error(`unhandled variant: ${JSON.stringify(value)}`)
}
