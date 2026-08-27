import { ReconnectingSocket } from "./reconnectingSocket"
import type { EventsClientMessage, EventsServerMessage } from "./types"

// The server silently rejects an interest frame carrying more than this many
// topics (its `MAX_EVENT_TOPICS_PER_FRAME`). The reconnect resend, which sends
// the WHOLE interest set, chunks into frames of at most this size so a client
// watching many sessions never loses its tail of subscriptions on reconnect.
const MAX_EVENT_TOPICS_PER_FRAME = 64

// JSON events channel. It tracks subscriptions, resends them in bounded chunks
// after every open, parses resource/control frames, and exposes connection state.
// It stays active while hidden so attention events can reach notifications;
// reconnect timing and lifecycle live in `ReconnectingSocket`.
export class EventsSocket extends ReconnectingSocket {
  // The complete, authoritative interest set. Coarse topics (sessions/projects/
  // config) plus the per-screen fine topics (session:<id>:changes) all live
  // here, so a reconnect can re-send the lot.
  private readonly subscriptions = new Set<string>()

  // Consumer callback. Default is a no-op so the store only wires what it needs.
  onEvent: (ev: EventsServerMessage) => void = () => {}

  // The current interest set (sorted for deterministic test assertions). Read
  // only — mutate via subscribe/unsubscribe.
  get topics(): string[] {
    return [...this.subscriptions].sort()
  }

  // Re-send the WHOLE interest set on every (re)open: server-side interest is
  // per-connection and a dropped connection discards it. Chunked so a large set
  // never exceeds the server's per-frame topic cap (which it would silently drop).
  protected onSocketOpen(): void {
    if (this.subscriptions.size > 0) {
      const topics = [...this.subscriptions]
      for (let i = 0; i < topics.length; i += MAX_EVENT_TOPICS_PER_FRAME) {
        this.sendRaw({
          subscribe: topics.slice(i, i + MAX_EVENT_TOPICS_PER_FRAME),
        })
      }
    }
  }

  protected handleMessage(event: MessageEvent): void {
    if (typeof event.data === "string") {
      this.handleText(event.data)
    }
  }

  private handleText(raw: string): void {
    let message: EventsServerMessage
    try {
      message = JSON.parse(raw) as EventsServerMessage
    } catch (err) {
      // Log the length but not the potentially large or sensitive frame body.
      console.warn(
        `[dux] events socket dropped an unparseable frame (${raw.length} chars)`,
        err,
      )
      return
    }
    // Forward resource and control frames as-is; the store dispatches on
    // `event`. Lag catch-up arrives as an ordinary `session.changes` frame.
    if (typeof message.event === "string") {
      this.onEvent(message)
    }
  }

  // Add topics to the interest set and (when open) send only the newly-added
  // ones. The full set is re-sent on the next open, so a subscribe issued while
  // the socket is down is not lost.
  subscribe(topics: string[]): void {
    const added: string[] = []
    for (const topic of topics) {
      if (!this.subscriptions.has(topic)) {
        this.subscriptions.add(topic)
        added.push(topic)
      }
    }
    if (added.length > 0) this.sendRaw({ subscribe: added })
  }

  // Remove topics from the interest set and (when open) tell the server. Removing
  // an unknown topic is a no-op.
  unsubscribe(topics: string[]): void {
    const removed: string[] = []
    for (const topic of topics) {
      if (this.subscriptions.delete(topic)) removed.push(topic)
    }
    if (removed.length > 0) this.sendRaw({ unsubscribe: removed })
  }

  private sendRaw(message: EventsClientMessage): void {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(message))
    }
  }
}
