---
title: Reaching dux over Tailscale
description: How dux finds and binds your Tailscale address, what happens when Tailscale isn't there, why a MagicDNS name needs allowed_hosts, what plain HTTP costs you in the browser, how to put Tailscale's HTTPS proxy in front, and the caveats worth knowing before you open your agents to a tailnet.
group: Web UI
order: 65
---

This is the feature that makes dux on a phone actually pleasant. Tailscale gives your
machine a stable private address that follows it between networks, dux binds that
address by default, and your phone opens a URL. No port forwarding, no dynamic DNS,
no VPN client to babysit.

It is also the point where dux stops being reachable only by you, so read
[the trust model](/docs/server-mode#the-trust-model-stated-plainly) before you rely
on it. There is no login.

## What dux actually does

Tailscale binding is **on by default** and is a single opt-out switch:

```toml
[server]
tailscale_enabled = true
```

When it is on, and you have not passed `--no-tailscale`, dux runs the `tailscale ip`
command and reads the address back. It takes the first IPv4 in Tailscale's
`100.64.0.0/10` range, falling back to the first IPv6 in Tailscale's own
`fd7a:115c:a1e0::/48` block. A plain LAN address or a link-local one is ignored, so
dux cannot accidentally bind something that merely looks similar.

That address then joins the **listen plan** as an extra leg, at the *same port* as
your primary address. It never replaces your `host`. So the default is two listeners,
loopback and tailnet, one URL each, both printed in the startup banner with the
tailnet row labelled `Tailscale` and a note that says other devices on your tailnet
can reach it with no login. dux skips the extra leg when it would be redundant,
meaning when your primary is already `0.0.0.0` or already that same address.

The two legs are not equal in one respect, and it is deliberate: your configured
address is **required** and a failure to bind it is fatal, while the Tailscale leg is
**best effort**. If something else is already on that port on the tailnet address,
dux warns and keeps serving the addresses that did bind rather than refusing to
start.

### When Tailscale isn't there

Nothing breaks. Detection failing is a warning, never a fatal error, and dux serves
your configured host regardless. There are three distinct cases and the warning names
which one you hit:

- the `tailscale` CLI is not installed or not on `PATH`
- the CLI ran and failed, which is what a stopped daemon or a logged-out node looks
  like
- the CLI ran fine and returned nothing dux could use

In every case you get one warning and a working server. If you do not use Tailscale
and would rather not read about it every start, set `tailscale_enabled = false` (or
pass `--no-tailscale` for a single run) and the warning goes with it.

### The palette flip is loopback plus Tailscale, always

Worth filing away, because it surprises people: `dux server` honors your configured
`[server] host` and `--bind`, but flipping a running terminal UI into the browser
with the `start-web-server` command always serves loopback plus your Tailscale
address, and never reaches for a custom host. If you need a specific interface, start
with `dux server`.

## The MagicDNS gotcha

Both ways in work: the raw `100.x` address and the MagicDNS name. This is the single
most common Tailscale support question because they do not need the same amount of
setup, so here it is up front.

**The `100.x` address works with no configuration. A MagicDNS hostname works too,
but returns `403` until you allow it.**

dux runs a Host-header allowlist in front of everything, which is what stops a
malicious web page from DNS-rebinding your browser into your server. It accepts
`localhost` and any loopback address, and it accepts a Host that is an IP literal it
actually bound. Your tailnet `100.x` address is exactly that, so
`http://100.101.102.103:8080` just works.

A MagicDNS name like `box.tailnet.ts.net` is a hostname, not an IP literal, and a
hostname is never something dux bound, so it fails the check and you get a plain
`403` reading *"this dux server does not serve the requested host"*. Nothing is
broken and nothing about the name is second class; you simply have to say it out
loud once:

```toml
[server]
allowed_hosts = ["box.tailnet.ts.net"]
```

Hostnames only, no scheme and no port. Entries are matched case-insensitively and
the port is ignored, so one entry covers every port you might serve on. A trailing
dot is stripped too, so `box.tailnet.ts.net.` matches the same entry. There is no
wildcard: `"*"` is treated as a literal hostname and will not match anything.

dux's other browser defense, a same-origin check on socket upgrades and write
requests, needs nothing from you here. It compares the request's `Origin` against its
`Host`, and a browser sitting at your tailnet URL sends a matching pair.

## Plain HTTP costs you a few browser features

dux serves plain HTTP only. There is no built-in TLS, by design: certificates are
delegated to a proxy in front of it if you want them. A tailnet address over plain
HTTP is not a "secure context" as browsers define it, and browsers switch off a
handful of APIs there. What that actually costs you:

- **Right-click paste stops working.** Reading your clipboard needs a secure context.
  dux notices and toasts a hint pointing you at `Ctrl+v` instead, rather than failing
  silently.
- **`Ctrl+v` still works**, which is why that is the hint. dux intercepts the chord
  and lets the browser's native paste event feed the terminal, and that path needs no
  secure context at all.
- **Copying still works.** Select-to-copy, the press-and-hold selection you use on
  a phone, the copy chords, and "copy local path" all fall back to the legacy copy
  path inside your click or your touch, so they are unaffected.
- **An agent writing your clipboard (`OSC 52`) silently does nothing.** The
  `clipboard_passthrough` setting still reads as enabled and no error appears, so if
  you are wondering why an agent's clipboard write never landed, this is why.
- **Desktop notifications are unavailable.** Browsers only allow the notification
  permission prompt on a secure origin. dux degrades quietly: the **Enable browser
  notifications** row hides itself when the browser exposes no notification API, and
  nothing fires. The **Desktop notifications** preference stays visible and
  toggleable regardless, so it can look armed while being inert.
- **Installing dux as an app is unavailable.** The PWA manifest ships, but browsers
  require a secure origin to offer installation, and dux's service worker
  deliberately stays dormant off a secure context. The only thing that worker ever
  did was serve a branded "dux is unreachable" page for a navigation made while the
  server is down; the in-app "Reconnecting…" overlay is plain React and works fine.
- **Everything that matters still works.** The terminal, the live WebSocket streams,
  the file editor, git, macros, the mobile compose bar. The socket URL is derived from
  the page, so plain HTTP simply yields `ws://`.

One more that is *not* a TLS problem but bites the same person: the editor's **Open
editor** button, which hands a path to the editor on the machine you are sitting at,
is disabled on any tailnet address. That is a deliberate host check, because a remote
URL means "your editor is not on that machine," and no certificate would change it.

If those tradeoffs bother you, terminate TLS in a reverse proxy in front of dux and
add its hostname to `allowed_hosts`. Tailscale ships one, and
[the recipe is below](#putting-the-tailscale-https-proxy-in-front).

## Caveats worth knowing

**There is no login, so routing is your only access control.** Anyone who can reach
the port has your whole workspace: every agent, every terminal, every worktree, and
the server's filesystem through the project picker. Because the terminal is
read-write for whoever holds input, that includes typing into a session you are in
the middle of using. Treat "on my tailnet" as "holding a terminal on this machine".

**"My tailnet" is probably wider than you think.** Tailscale's default policy is
allow-all: every device of every member can reach every other device, on every port,
until someone edits the ACL. If your tailnet has other people in it, or a device you
would not hand a shell to, restrict dux's port in your policy file before you rely on
this.

**Node keys expire.** Tailscale expires device keys on a schedule by default, and an
expired node quietly drops off the tailnet, which looks exactly like dux being
broken when it is not. If this machine is something you expect to reach at 2am from a
phone, disable key expiry for it in the Tailscale admin console.

**Do not use Tailscale Funnel for this.** Funnel publishes a service to the anonymous
public internet. dux has no login. Those two facts do not belong in the same sentence,
and dux offers no support for it.

## Putting the Tailscale HTTPS proxy in front

Tailscale can terminate HTTPS for a local service, which buys back the clipboard and
notification features listed above, because `https://box.tailnet.ts.net` is a secure
context and a plain tailnet IP is not. dux needs no TLS setup of its own for this. It
keeps serving plain HTTP on loopback and Tailscale owns the certificate.

Serve dux on loopback, then point the proxy at that port:

```bash
dux server --bind 127.0.0.1:8080
tailscale serve --bg 8080
```

Then allow the node's MagicDNS name, which is the same single line the plain MagicDNS
path needs:

```toml
[server]
allowed_hosts = ["box.tailnet.ts.net"]
```

Open `https://box.tailnet.ts.net` and you are done. The socket URLs are derived from
the page, so they become `wss://` on their own with nothing to configure.

Note that the Tailscale leg is still added on top of your `--bind` address, so dux is
also answering plain HTTP directly on `100.x:8080` alongside the proxied URL. If you
want the HTTPS path to be the only way in, add `--no-tailscale` to the `dux server`
line and let the proxy own the tailnet side.

One honest clause: the plain address is the path the maintainer uses daily, and this
proxy path is documented from dux's behaviour rather than from a tested recipe. If it
does not work, two things are worth checking first, and they fail in very different
ways.

**Check that the proxy passes WebSockets through.** dux is not merely degraded
without them, it is unusable: every terminal rides a WebSocket, and so does the
change feed that tells the page when anything moved. This is the confusing failure, because nothing returns an error
you can see. The page loads, looks right, and then nothing is live: no output, no
status changes, and the in-app *Reconnecting…* overlay sitting there. If the UI
arrives but never comes alive, suspect the upgrade rather than dux.

**Check the `Host` and `Origin` the proxy sends.** dux runs two separate checks. The
host allowlist tests `Host` on every request, and a same-origin check compares the
`Origin`'s host and port against `Host` on every WebSocket upgrade and every write
request. A proxy that forwards the original `Host` satisfies both, once that hostname
is in `allowed_hosts`. A proxy that rewrites `Host` to its backend target
(`127.0.0.1:8080`) passes the allowlist, since loopback is always allowed, and then
fails the origin check, because the browser still sends the external name as
`Origin`. So preserving the original `Host` is the fix; adding a rewritten one to
`allowed_hosts` only silences the first check and leaves the second.

The `403` is the friendly failure of the two, and the response body says which check
fired: *"this dux server does not serve the requested host"* is the allowlist, and
*"cross-origin WebSocket upgrade rejected"* (or *"cross-origin request rejected"* on
a write) is the same-origin check. Either way you know exactly what to change.

## Where to go next

- [Server mode overview](/docs/server-mode): the `[server]` keys in full, the startup
  banner, graceful shutdown, and the trust model.
- [The workspace in the browser](/docs/web-workspace): what you actually get once you
  are in, including the phone layout.
- [Hosting dux behind a login](/docs/public-hosting): the other answer, for when the
  machine is not on a private network and something has to ask who you are first.
