---
title: Reaching dux over Tailscale
description: How dux finds and binds your Tailscale address, why a MagicDNS name needs allowed_hosts, what plain HTTP costs you in the browser, how to put Tailscale's HTTPS proxy in front, and the caveats before you open your agents to a tailnet.
group: Web UI
order: 65
---

Tailscale gives your machine a stable private address that follows it between networks,
dux binds that address by default, and your phone opens a URL. No port forwarding, no
dynamic DNS, no VPN client to babysit.

> [!WARNING]
> This is the point where dux stops being reachable only by you. **There is no login.**
> Read [the trust model](/docs/server-mode#the-trust-model-stated-plainly) first.

## What dux actually does

Tailscale binding is **on by default**, and the setting has three answers:

```toml
[server]
tailscale = "auto"   # or "yes", or "no"
```

- **`"auto"`** (the default) binds your Tailscale address whenever it exists, and keeps
  looking. This is the one you want on a laptop; see
  [it follows the interface](#it-follows-the-interface).
- **`"yes"`** looks exactly once and never again by itself. If Tailscale is not up at
  that moment, dux serves your configured host only until you change the mode.
- **`"no"`** never binds it and never runs the detection at all.

`dux server --no-tailscale` forces `"no"` for a single run.

> [!NOTE]
> The old boolean `tailscale_enabled` still works and is rewritten for you: `true` becomes
> `"yes"` and `false` becomes `"no"`. The next time dux saves your config the old line is
> gone.

When the mode is not `"no"`, dux runs `tailscale ip` and takes the first IPv4 in
Tailscale's `100.64.0.0/10` range, falling back to the first IPv6 in Tailscale's own
`fd7a:115c:a1e0::/48` block. A plain LAN or link-local address is ignored.

That address joins the listen plan as an extra leg at the *same port* as your primary
address, and never replaces your `host`. So the default is two listeners, loopback and
tailnet, one URL each, both printed in the startup banner with the tailnet row labelled
`Tailscale` and a note that other tailnet devices can reach it with no login. dux skips the
extra leg when your primary is already `0.0.0.0` or already that same address.

> [!IMPORTANT]
> The two legs are not equal. Your configured address is **required**, and failing to bind
> it is fatal. The Tailscale leg is **best effort**: if something else already holds that
> port, dux warns and keeps serving the addresses that did bind.

### It follows the interface

A tailnet address appears when Tailscale connects and vanishes when it does not, which on
a roaming laptop is several times a day. On `"auto"`:

- **The interface appears** and dux binds it mid-run, with no restart. A new URL starts
  answering and everything else is untouched.
- **The interface goes away** and dux drops that one listener. Your configured address keeps
  serving throughout. Pages open over the tailnet show the in-app *Reconnecting…* overlay
  and pick up again by themselves when the address comes back, with their terminals still
  scrolled where you left them.
- **Your Tailscale address changes** and dux moves the listener to the new one.

dux checks roughly every ten seconds, which is not configurable. That interval is also the
only debounce, so an interface that flaps faster than dux looks costs at most one bind or
unbind. Every bind and unbind is written to `dux.log`, printed by `dux server`, and listed
in the flip's activity panel.

> [!TIP]
> The mode itself is a live switch: change it from the terminal UI's palette, the
> browser's Preferences dialog, or by editing the file and reloading your config, and the
> listener follows without a restart. See
> [Changing the mode while dux is serving](#changing-the-mode-while-dux-is-serving).

### When Tailscale isn't there

Nothing breaks. Detection failing is a warning, never fatal, and dux serves your configured
host regardless. Three distinct cases, and the warning names which one you hit:

- the `tailscale` CLI is not installed or not on `PATH`
- the CLI ran and failed, which is what a stopped daemon or a logged-out node looks like
- the CLI ran fine and returned nothing dux could use

A fourth folds into the second: if the daemon stops answering entirely, dux's call is capped
at a few seconds, killed, and reported as a failure rather than hanging.

The warning also says what happens next, which differs by mode: on `"auto"` it is a "not
yet" and dux keeps looking, while on `"yes"` it is settled for the rest of the run. If you
do not use Tailscale, set `tailscale = "no"` (or pass `--no-tailscale`) and the warning goes
with it.

### Serving from the terminal UI is loopback plus Tailscale, always

`dux server` honors your configured `[server] host` and `--bind`. The two ways of serving
from inside a running terminal UI always serve loopback plus your Tailscale address and
never reach for a custom host: that is both flipping with `start-web-server` and keeping the
TUI with `serve_while_tui` (see
[Serve in the background](/docs/server-mode#serve-in-the-background-and-keep-the-tui)). If
you need a specific interface, start with `dux server`.

The `tailscale` mode applies to all three the same way, watcher included. On `"auto"` a
flipped server picks up your tailnet address while its status screen sits there and says so
in the activity panel, and a background server does it while you work in the TUI.

### Changing the mode while dux is serving

You do not have to edit `config.toml` and restart. In the terminal UI the palette's
**set-tailscale-mode** opens a three-row picker; in the browser it is the **Bind your
Tailscale address** row in the Preferences dialog. Both save the value to
`config.toml` **and** apply it to the listener that is serving right now, and both
tell you what actually happened rather than just "saved".

- Choosing **`"no"`** stops the interface watcher and drops the Tailscale listener.
  Anything connected over your tailnet loses its connection, including the browser
  tab you clicked in. That is allowed on purpose, and the row says so: reopen dux on
  its other address.
- Choosing **`"yes"`** looks for the address right then. If nothing is found you get
  a warning rather than an error, and the value still saves.
- Choosing **`"auto"`** starts the watcher and probes immediately, so you see the
  outcome now instead of up to ten seconds later.

If nothing is serving, the choice is simply saved and applied the next time a
listener starts, and the message says so. A run started with `dux server
--no-tailscale` refuses a live change for as long as it lasts, because the flag
outranks the file; the value still saves for the next run. Editing the file and
running `reload-config` is live too, and takes the same path.

## The MagicDNS gotcha

> [!IMPORTANT]
> The raw `100.x` address works with no configuration. A **MagicDNS hostname works too, but
> returns `403` until you allow it.**

dux runs a Host-header allowlist in front of everything, which is what stops a malicious web
page from DNS-rebinding your browser into your server. It accepts `localhost` and any
loopback address, a Host that is an IP literal it actually bound, and, unless the mode is
`"no"`, any IP literal inside Tailscale's own ranges. So `http://100.101.102.103:3890` just
works, and it keeps working even while the Tailscale leg is down.

A MagicDNS name like `box.tailnet.ts.net` is a hostname, never something dux bound, so it
fails the check and you get a plain `403` reading *"this dux server does not serve the
requested host"*. Say the name out loud once:

```toml
[server]
allowed_hosts = ["box.tailnet.ts.net"]
```

Hostnames only, no scheme and no port. Entries are matched case-insensitively and the port
is ignored, so one entry covers every port. A trailing dot is stripped, so
`box.tailnet.ts.net.` matches the same entry. There is no wildcard: `"*"` is a literal
hostname and matches nothing.

dux's other browser defense, a same-origin check on socket upgrades and write requests,
needs nothing from you here: a browser sitting at your tailnet URL sends a matching `Origin`
and `Host`.

## Plain HTTP costs you a few browser features

dux serves plain HTTP only, with no built-in TLS: certificates are delegated to a proxy in
front of it. A tailnet address over plain HTTP is not a "secure context" as browsers define
it, and browsers switch off a handful of APIs there:

- **Right-click paste stops working.** Reading your clipboard needs a secure context. dux
  toasts a hint pointing you at `Ctrl+v` instead, rather than failing silently.
- **`Ctrl+v` still works.** dux intercepts the chord and lets the browser's native paste
  event feed the terminal, which needs no secure context.
- **Copying still works.** Select-to-copy, press-and-hold selection on a phone, the copy
  chords, and "copy local path" all fall back to the legacy copy path inside your click or
  touch.
- **An agent writing your clipboard (`OSC 52`) silently does nothing.** The
  `clipboard_passthrough` setting still reads as enabled and no error appears.
- **Desktop notifications are unavailable.** The **Enable browser notifications** row hides
  itself when the browser exposes no notification API, and nothing fires. The **Desktop
  notifications** preference stays visible and toggleable regardless, so it can look armed
  while being inert.
- **Installing dux as an app is unavailable.** The PWA manifest ships, but browsers require a
  secure origin to offer installation, and dux's service worker stays dormant off one. All
  that worker ever does is serve a branded "dux is unreachable" page for a navigation made
  while the server is down; the in-app "Reconnecting…" overlay is plain React and works fine.
- **Everything that matters still works.** The terminal, the live WebSocket streams, the file
  editor, git, macros, the mobile compose bar. The socket URL is derived from the page, so
  plain HTTP yields `ws://`.

One more that is *not* a TLS problem: the editor's **Open local editor** button, which hands a path
to the editor on the machine you are sitting at, is disabled on any tailnet address. That is
a host check, not a certificate one.

> [!TIP]
> If those tradeoffs bother you, terminate TLS in a reverse proxy in front of dux and add its
> hostname to `allowed_hosts`. Tailscale ships one, and
> [the recipe is below](#putting-the-tailscale-https-proxy-in-front).

## Caveats worth knowing

> [!CAUTION]
> **There is no login, so routing is your only access control.** Anyone who can reach the
> port has your whole workspace: every agent, every terminal, every worktree, and the
> server's filesystem through the project picker. The terminal is read-write for whoever
> holds input, so that includes typing into a session you are in the middle of using. Treat
> "on my tailnet" as "holding a terminal on this machine".

**"My tailnet" is probably wider than you think.** Tailscale's default policy is allow-all:
every device of every member can reach every other device, on every port, until someone
edits the ACL. If your tailnet has other people in it, restrict dux's port in your policy
file first.

**On `"auto"`, "reachable on my tailnet" is a standing fact, not a snapshot.** dux being
loopback-only right now does not mean it will stay that way: the listener comes back with the
interface, without asking. For a run that can never grow that address, use `--no-tailscale`.

**Node keys expire.** Tailscale expires device keys on a schedule by default, and an expired
node quietly drops off the tailnet, which looks exactly like dux being broken. If you expect
to reach this machine at 2am from a phone, disable key expiry for it in the Tailscale admin
console.

> [!CAUTION]
> **Do not use Tailscale Funnel for this.** Funnel publishes a service to the anonymous
> public internet, and dux has no login. dux offers no support for it.

## Putting the Tailscale HTTPS proxy in front

Tailscale can terminate HTTPS for a local service, which buys back the clipboard and
notification features above, because `https://box.tailnet.ts.net` is a secure context and a
plain tailnet IP is not. dux needs no TLS setup of its own.

Serve dux on loopback, then point the proxy at that port:

```bash
dux server --bind 127.0.0.1:3890
tailscale serve --bg 3890
```

Then allow the node's MagicDNS name:

```toml
[server]
allowed_hosts = ["box.tailnet.ts.net"]
```

Open `https://box.tailnet.ts.net` and you are done. The socket URLs are derived from the
page, so they become `wss://` on their own.

The Tailscale leg is still added on top of your `--bind` address, so dux is also answering
plain HTTP directly on `100.x:3890` alongside the proxied URL. To make the HTTPS path the
only way in, add `--no-tailscale` to the `dux server` line and let the proxy own the tailnet
side.

> [!NOTE]
> The plain address is the path the maintainer uses daily. This proxy path is documented from
> dux's behaviour rather than from a tested recipe.

If it does not work, check two things.

**Does the proxy pass WebSockets through?** Every terminal rides a WebSocket, and so does the
change feed that tells the page when anything moved, so without them dux is unusable rather
than degraded. Nothing returns an error you can see: the page loads, looks right, and then
nothing is live, with the in-app *Reconnecting…* overlay sitting there.

**What `Host` and `Origin` does the proxy send?** The host allowlist tests `Host` on every
request, and a same-origin check compares the `Origin`'s host and port against `Host` on
every WebSocket upgrade and every write request. A proxy that forwards the original `Host`
satisfies both, once that hostname is in `allowed_hosts`. A proxy that rewrites `Host` to its
backend target (`127.0.0.1:3890`) passes the allowlist, since loopback is always allowed, and
then fails the origin check, because the browser still sends the external name as `Origin`.
Preserving the original `Host` is the fix; adding a rewritten one to `allowed_hosts` only
silences the first check.

The response body of a `403` says which check fired: *"this dux server does not serve the
requested host"* is the allowlist, and *"cross-origin WebSocket upgrade rejected"* (or
*"cross-origin request rejected"* on a write) is the same-origin check.

## Where to go next

- [Server mode overview](/docs/server-mode): the `[server]` keys in full, the startup banner,
  graceful shutdown, and the trust model.
- [The workspace in the browser](/docs/web-workspace): what you get once you are in,
  including the phone layout.
- [Hosting dux behind a login](/docs/public-hosting): the other answer, for when the machine
  is not on a private network.
