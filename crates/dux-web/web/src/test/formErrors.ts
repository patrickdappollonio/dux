// The development errors `FormGroup` and `FormSection` print, kept as they are
// printed so the setup file can fail the test that raised them.
const raised: string[] = []
const watching = Symbol.for("dux.formErrors")

export function watchFormErrors(): void {
  const current = console.error as typeof console.error & { [watching]?: true }
  if (current[watching]) return
  const watched = (...args: unknown[]) => {
    if (typeof args[0] === "string" && /^Form(?:Group|Section): /.test(args[0])) raised.push(args[0])
    current(...args)
  }
  console.error = Object.assign(watched, { [watching]: true as const })
}

/** The errors raised since the last call, which it forgets. */
export function takeFormErrors(): string[] {
  return raised.splice(0)
}
