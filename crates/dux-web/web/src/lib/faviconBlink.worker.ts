// The favicon blink's clock, run in a dedicated worker. The page asks for one
// wake at a time; each request replaces the last. The page decides what to show,
// this only says "now". See `faviconBlink.ts` for why the clock lives here.

type Request = { type: "set"; id: number; delay: number } | { type: "clear" }

// The app compiles against the DOM lib, where `self` is a Window whose
// `postMessage` wants a target origin; in a worker it takes the message alone.
const scope = self as unknown as {
  onmessage: ((ev: MessageEvent<Request>) => void) | null
  postMessage(message: unknown): void
}

let pending: ReturnType<typeof setTimeout> | undefined

scope.onmessage = (ev) => {
  if (pending !== undefined) clearTimeout(pending)
  pending = undefined
  const msg = ev.data
  if (msg.type === "set") {
    const { id } = msg
    pending = setTimeout(() => {
      pending = undefined
      scope.postMessage({ type: "tick", id })
    }, msg.delay)
  }
}
