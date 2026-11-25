# Web server

The browser UI has its own rules in `web/AGENTS.md`.

## Trust model

- The server is single-tenant: every client shares one workspace and can drive any PTY, browse the server's filesystem and see every session. Never add a feature that assumes mutually distrusting web users without first designing per-user isolation.
- Its one gate is an optional single-owner password in `[server.auth]`, stored only as an Argon2id hash; there are no per-user accounts.
- `[server.auth]` fails closed: an unreadable section stops dux from starting and stops a reload from changing anything.
- One auth layer in `src/auth/` classifies every request, by the listener it arrived on and only what the nearest hop can vouch for; the declared public routes are the only exceptions.
- Never trust a client-sent `Tailscale-*` header; parse every header the auth layer reads (forwarding headers, Host, Origin, Cookie, the Tailscale headers) from its raw bytes, entry by entry.
- Bans are written as single addresses and as ranges, never as a long list of addresses: a /64 whose addresses together reach the limit becomes one range entry that replaces the single entries inside it.
- Loopback and this machine's own addresses are never banned.
- The server binds loopback by default and optionally a Tailscale address; a Host-header allowlist and a same-origin check guard every request.
- Every WebSocket closes the moment its session ends, and PTY and events sockets re-check the session before their opening frames.

## Serving

- Every running dux serves the API on its control socket; the web is served by `dux server`, by the TUI's `start-web-server` flip, or in the TUI's background with `[server] serve_while_tui`.
- The two in-app ways bind loopback and the Tailscale leg only and never read `[server] host`.
- `[server] serve_while_tui` and `[server] tailscale` apply live; `host`, `port` and the caps need a restart.
- A Tailscale mode change goes through `TailscaleModeControl`, and only the serve loop acts on it.
- Each serving mode has exactly one config reload owner: the engine actor for `dux server` and the flip, the TUI's `apply_reloaded_config` for the background mode.

## Who drives a row

- Every wire command is judged in `Engine::apply_wire` through `row_change`, and every engine-actor request through `request_row_change`; both matches stay exhaustive, with no wildcard arm.
- A git or file write the web layer makes itself asks `Engine::admit_write` and checks its destinations against the folders it returns, compared canonically.
- A request that is not a plain read names its page in `x-dux-page`; the page drives a row when the row's driver is the seat that page's sockets share.
- A refused change answers `423 Locked` with one sentence that names the driver and says nothing was changed.
- `force_connected=true` (`--dangerously-ignore-connected` on the command line) goes ahead over the driver; a list of accepted blockers does not.
- Only a resize flagged as a take-over transfers ownership; the claim decision and the resize enqueue share one critical section.
- An automatic reconnect never carries the take-over flag, and losing ownership is sticky: nothing passive re-claims a row.

## Counts

- Every viewer count counts distinct browsers and is worded "browser" or "browsers"; it is zero when nothing is serving.
- The browser id is display-only: it feeds counts and the connections listing, and never ownership, auth, presence grace or a destructive-change decision.
