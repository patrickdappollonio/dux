// A bound on one request. The request gets a signal it should honour, and the
// wait ends at the deadline whether it honours it or not, so a server that
// accepts the connection and never answers cannot wedge whoever is waiting.

export class DeadlineError extends Error {
  constructor() {
    super("dux did not answer in time.")
    this.name = "DeadlineError"
  }
}

export function isDeadline(e: unknown): e is DeadlineError {
  return e instanceof DeadlineError
}

export async function withDeadline<T>(
  ms: number,
  run: (signal: AbortSignal) => Promise<T>,
): Promise<T> {
  const controller = new AbortController()
  let timer: ReturnType<typeof setTimeout> | undefined
  const expired = new Promise<never>((_resolve, reject) => {
    timer = setTimeout(() => {
      controller.abort()
      reject(new DeadlineError())
    }, ms)
  })
  try {
    return await Promise.race([run(controller.signal), expired])
  } catch (e) {
    if (controller.signal.aborted) throw new DeadlineError()
    throw e
  } finally {
    clearTimeout(timer)
  }
}
