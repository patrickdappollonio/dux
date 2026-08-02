// The exhaustiveness helper. Call it in the default/fall-through position of a
// switch over a discriminated union: TypeScript narrows the value to `never`
// once every variant is handled, so the call type-checks. Add a variant to the
// union without adding its case and the value is no longer `never`, so `tsc`
// fails at the call site.
//
// This is a COMPILE-time guard, which a runtime `default: throw` is not: a
// runtime throw compiles perfectly happily with a case missing and only tells
// you at the moment a user hits it. That is precisely the failure this exists to
// prevent, so prefer this everywhere a union decides behaviour.
//
// It still throws, because a value reaching here at runtime means the data did
// not match the type (an older/newer server, hand-written JSON), and failing
// loudly beats carrying on with a value nothing understands.
export function assertNever(value: never): never {
  throw new Error(`unhandled variant: ${JSON.stringify(value)}`)
}
