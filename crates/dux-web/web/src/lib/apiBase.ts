// Where the server lives, for every HTTP request and every WebSocket the page
// opens. The one place that knows: a UI served from somewhere other than the
// dux it talks to changes these two functions and nothing else.
//
// Today the page is always served by the dux it drives, so API paths stay
// relative and sockets go to the page's own host. Both read `location` at call
// time rather than at import, so tests can stub it and nothing captures a stale
// copy.

/// The URL for an API path (`/api/v1/...`, query string included).
export function apiUrl(path: string): string {
  return path
}

/// The URL for a WebSocket path (`/ws/...`). The scheme follows the page's:
/// a plain `ws://` from an HTTPS page is blocked as mixed content.
export function wsUrl(path: string): string {
  const scheme = location.protocol === "https:" ? "wss:" : "ws:"
  return `${scheme}//${location.host}${path}`
}
