---
title: Reaching dux over Tailscale
description: How dux finds and binds your Tailscale address, answers to your MagicDNS name, picks up tailscale serve for HTTPS and shows QR codes for your phone, what plain HTTP costs you in the browser, what to check when the name does not resolve, and the caveats before you open your agents to a tailnet.
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
- **`"no"`** never binds it and never runs the detection at all, which also turns off
  dux's checks for a Funnel publishing it (see
  [When Tailscale isn't there](#when-tailscale-isnt-there)).

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

dux checks roughly every five seconds, which is not configurable. That interval is also the
only debounce, so an interface that flaps faster than dux looks costs at most one bind or
unbind. Every bind and unbind is written to `dux.log`, printed by `dux server`, listed in
the flip's log viewer, and said on screen: the terminal UI's status line and the
browser's toasts both name the address that arrived or went away, and both tell you when a
bind keeps failing. On screen the news waits until the interface has stayed one way for a
few seconds, so an interface that keeps flapping gets one message saying so and one more
once it settles, rather than a new one every time it moves.

> [!TIP]
> The mode itself is a live switch: change it from the terminal UI's palette, the
> browser's Preferences dialog, or by editing the file and reloading your config, and the
> listener follows without a restart. See
> [Changing the mode while dux is serving](#changing-the-mode-while-dux-is-serving).

### When Tailscale isn't there

Unless the mode is `"no"`, dux asks Tailscale on every look whether a Funnel publishes it
(see [the caution below](#caveats-worth-knowing)), and it serves only when it gets an
answer. dux runs the `tailscale` command, looking for it on your `PATH`, in
`/usr/local/bin`, and inside the macOS app (`/Applications/Tailscale.app`, always as the
command line tool, never opening the app). What happens next is one of three things.

**dux serves, because nothing can publish it.** Two answers count, and both only when
nothing of Tailscale is on this machine at all: no address on Tailscale's own network
interface, no Tailscale daemon socket, and no running `tailscaled`.

- The `tailscale` command is not in any of those places.
- The `tailscale` command says, in its own words, that the daemon is not running.

Other VPNs that hand out addresses in the same `100.x` range (Cloudflare WARP, NetBird) are
not Tailscale and do not count.

**dux refuses every request, because it cannot confirm.** Every page answers `503` saying
dux could not confirm that no Funnel publishes it, and the log says why:

- the command failed, timed out (dux caps each call at a few seconds and kills it), or
  could not reach a daemon that may well be there;
- the command answered with nothing dux could read;
- the command is missing or says the daemon is down, yet something of Tailscale is here:
  an address on its interface, a daemon socket, or a running `tailscaled`.

**A Funnel already seen stays refused** through all of it. Once dux has seen a Funnel to its
port, a daemon that stops or a command that fails changes nothing; only a look that shows the
Funnel gone brings dux back.

How to get out, in order:

1. **Fix `tailscaled`.** `tailscale status` on the machine dux runs on says what is wrong.
   Every look that succeeds lifts the refusal by itself.
2. **Put the `tailscale` command where dux looks**, typically on `PATH`, when the daemon is
   running but dux cannot find its command.
3. **Set `tailscale = "no"`** (or pass `--no-tailscale`) when you do not want dux consulting
   Tailscale at all. That turns off the Funnel checks above too: only Tailscale's own mark on
   Funnel traffic is still refused, so keep it for machines where you know no Funnel points
   at dux.

> [!IMPORTANT]
> **Userspace networking and sidecars.** A `tailscaled` started with
> `--tun=userspace-networking` (the default in Tailscale's container image) has no network
> interface, and in a sidecar setup the `tailscale` command may live in another container.
> dux still spots the daemon by its socket (`/var/run/tailscale/tailscaled.sock`,
> `/run/tailscale/tailscaled.sock`, `/var/run/tailscaled.socket`, `/tmp/tailscaled.sock`,
> or the path in `$TS_SOCKET`) or by its process, and refuses until it can ask. Make the
> `tailscale` command available to dux and able to reach that daemon: it looks for the
> socket at `/var/run/tailscale/tailscaled.sock`, so mount or link the sidecar's socket
> there. Or, when no Funnel can reach dux there, set `tailscale = "no"`.

On `"auto"` dux keeps looking every few seconds and follows whatever it finds; on `"yes"`
it looks for the address once but keeps checking for a Funnel the same way.

### Serving from the terminal UI is loopback plus Tailscale, always

`dux server` honors your configured `[server] host` and `--bind`. The two ways of serving
from inside a running terminal UI always serve loopback plus your Tailscale address and
never reach for a custom host: that is both flipping with `start-web-server` and keeping the
TUI with `serve_while_tui` (see
[Serve in the background](/docs/server-mode#serve-in-the-background-and-keep-the-tui)). If
you need a specific interface, start with `dux server`.

The `tailscale` mode applies to all three the same way, watcher included. On `"auto"` a
flipped server picks up your tailnet address while its status screen sits there and says so
in its log viewer, and a background server does it while you work in the TUI.

### Changing the mode while dux is serving

You do not have to edit `config.toml` and restart. In the terminal UI the palette's
**set-tailscale-mode** opens a three-row picker; in the browser it is the **Bind your
Tailscale address** row in the Preferences dialog. Both save the value to
`config.toml` **and** apply it to the listener that is serving right now, and both
tell you what actually happened rather than just "saved".

- Choosing **`"no"`** stops the interface watcher, drops the Tailscale listener, and
  stops dux answering to this machine's MagicDNS name.
  Anything connected over your tailnet loses its connection, including the browser
  tab you clicked in. That is allowed on purpose, and the row says so: reopen dux on
  its other address.
- Choosing **`"yes"`** looks for the address, and reads the MagicDNS name and any
  `tailscale serve` route, right then. If no address is found you get
  a warning rather than an error, and the value still saves.
- Choosing **`"auto"`** starts the watcher and probes immediately, so you see the
  outcome now instead of up to five seconds later.

If nothing is serving, the choice is simply saved and applied the next time a
listener starts, and the message says so. A run started with `dux server
--no-tailscale` refuses a live change for as long as it lasts, because the flag
outranks the file; the value still saves for the next run. Editing the file and
running `reload-config` is live too, and takes the same path.

## Your MagicDNS name just works

Unless the mode is `"no"`, dux asks Tailscale for this machine's own MagicDNS name and
answers to it on any port, so `http://box.your-tailnet.ts.net:3890` opens dux with no
configuration at all. The `dux server` banner lists that URL beside the listener
addresses, and so do the addresses the terminal UI shows while it serves.

- **Only this machine's name.** Another machine on the same tailnet, a subdomain of this
  name, or any other `.ts.net` name still gets a plain `403` reading *"this dux server
  does not serve the requested host"*.
- **It follows a rename.** dux reads the name again every five seconds or so, on `"yes"`
  as well as `"auto"`, so renaming the machine or the whole tailnet moves it with no
  restart: the new name starts working and the old one stops.
- **Only a name Tailscale assigned.** The name must sit under your tailnet's `.ts.net`
  domain. A tailnet run by another control server, such as Headscale, uses a domain its
  operator chose, and that name goes in `allowed_hosts` like any other.
- **It fails closed.** If a look at Tailscale fails, dux stops answering to the name until
  the next look succeeds, a few seconds later at most.
- **`"no"` turns it off** along with the Tailscale listener.

> [!NOTE]
> This is safe for the same reason the `100.x` addresses are. The host guard exists to stop
> a web page you visit from pointing a name it controls at your machine. Your MagicDNS name
> is handed out by your tailnet's administrator, and no web page can claim it. It is safe
> only while the name stays on your tailnet, which is why Funnel turns it off (see the
> caveats below).

dux runs a Host-header allowlist in front of everything, and it also accepts `localhost`,
any loopback address, an IP literal dux actually bound, and, unless the mode is `"no"`,
any IP literal inside Tailscale's own ranges. So `http://100.101.102.103:3890` works too,
even while the Tailscale listener is down.

Any other name, such as a reverse proxy's hostname, goes in `allowed_hosts`:

```toml
[server]
allowed_hosts = ["dux.example.com"]
```

Hostnames only, no scheme and no port. Entries are matched case-insensitively and the port
is ignored, so one entry covers every port. A trailing dot is stripped. There is no
wildcard: `"*"` is a literal hostname and matches nothing. Edit the file and run
**Reload config**, and the running server answers by the new list with no restart.

dux's other browser defense, a same-origin check on socket upgrades and write requests,
needs nothing from you here: a browser sitting at your tailnet URL sends a matching `Origin`
and `Host`.

## HTTPS with `tailscale serve`

Tailscale can put a real certificate in front of dux, which buys back the clipboard and
notification features [plain HTTP costs you](#plain-http-costs-you-a-few-browser-features),
because `https://box.your-tailnet.ts.net` is a secure context and a plain tailnet address is
not. dux needs no TLS setup of its own.

1. In the Tailscale admin console, under **DNS**, make sure **MagicDNS** is on and turn on
   **HTTPS Certificates**.
2. Point Tailscale at dux's port, on the machine dux runs on:

   ```bash
   tailscale serve --bg 3890
   ```

3. Open `https://box.your-tailnet.ts.net` on any device on your tailnet.

dux notices the route by itself, within about five seconds, and from then on
lists the `https://` URL with its other addresses: in the `dux server` banner, in the
addresses the terminal UI shows, and in the QR codes below. Both the terminal UI's status
line and the browser's toasts say when a route to dux appears or goes away.

> [!IMPORTANT]
> **dux never runs `tailscale serve` for you.** It is a lasting change to the machine that
> may need an administrator and fails outright where HTTPS certificates are off, so it is
> yours to make. When MagicDNS is on and nothing serves dux yet, dux prints the exact
> command once, as a tip, and leaves it there.

> [!NOTE]
> The first HTTPS visit can take around 30 seconds while Tailscale gets the certificate
> issued. That happens once; later visits are immediate.

What counts as a route to dux: one at the root (`/`) that forwards to dux's port on this
machine (`127.0.0.1`, `localhost` or `[::1]`), which is exactly what
`tailscale serve --bg <port>` sets up. A route mounted under a path, such as
`--set-path /dux`, does not work, because dux's pages load everything from the root.
`tailscale serve status` lists what is configured, and `tailscale serve --https=443 off`
removes the route again (`tailscale serve reset` removes every route on the machine).

`tailscale serve` keeps the name and the `https` origin when it forwards a request, and it
carries WebSockets, so every terminal and the live change feed work through it as they do
over the plain address.

The plain Tailscale address keeps serving alongside the HTTPS one. To make HTTPS the only
way in from the tailnet, run with `tailscale = "no"` (or `dux server --no-tailscale`) and
list the name in `allowed_hosts`: `"no"` also stops dux answering to its name by itself,
and the explicit entry is what lets the HTTPS route through.

### QR codes for your phone

`dux server` and the [start-web-server flip](/docs/server-mode#flip-a-running-tui-into-the-browser)
show two QR codes, so a phone on your tailnet opens dux by pointing its camera at the
screen: the Tailscale IP address on the left, and the MagicDNS name on the right, which is
the `https://` URL when `tailscale serve` points at dux. Each code has its URL printed
under it. They sit side by side when the window is wide enough and stack when it is not,
and they appear once dux's first look at Tailscale has answered, then again whenever
those addresses change, so a rename or a new serve route gets a fresh pair.

![The dux server console: the banner lists the loopback and Tailscale addresses, the MagicDNS and HTTPS rows follow, and two QR codes sit side by side under them, one for http://100.101.102.103:3890 and one for https://demo-box.example-tailnet.ts.net.](/screens/server-qr-codes.png)

![The start-web-server flip's status screen: the dux logo, then every address dux answers on including http://demo-box.example-tailnet.ts.net:3890 and https://demo-box.example-tailnet.ts.net, and below it the Log panel with the same banner, MagicDNS and HTTPS rows and two QR codes side by side that dux server prints.](/screens/tui-flip-qr-codes.png)

`dux server` prints them only when its output is a terminal, so a log piped to a file stays
clean. The background server (`serve_while_tui`) never shows them, because the terminal UI
stays on screen. Set `qr_codes = false` under `[server]` to hide them everywhere; the
setting is read when a server starts.

## When the name does not answer

Work down this list; each check tells you which side the problem is on.

**MagicDNS is off for the tailnet.** `tailscale dns status` says MagicDNS is disabled
tailnet-wide. Turn it on in the admin console under **DNS**. Until then no `.ts.net` name
resolves, and dux offers no name URL or QR code for one.

**HTTPS certificates are off.** `tailscale serve` cannot get a certificate, and the
`CertDomains` list in `tailscale status --json` is empty. Turn on **HTTPS Certificates** in
the admin console under **DNS**.

**This device is not using Tailscale DNS.** Common on Linux, where it can be switched off.
`tailscale dns status` should report Tailscale DNS as enabled; turn it on with:

```bash
sudo tailscale set --accept-dns=true
```

(In the Tailscale app it is the **Use Tailscale DNS** setting.) On Linux with
systemd-resolved, `resolvectl status tailscale0` should then list `100.100.100.100` as a DNS
server.

**You use a custom resolver such as NextDNS.** That works when you set it as the tailnet's
global nameserver in the admin console: Tailscale's own resolver answers `.ts.net` names
itself and forwards everything else to yours. A device that skips Tailscale DNS and talks to
the custom resolver directly does not resolve `.ts.net` names at all.

**Telling "the name exists" from "this device cannot see it".** Ask both resolvers:

```bash
getent hosts box.your-tailnet.ts.net                   # what this device resolves
dig +short box.your-tailnet.ts.net @100.100.100.100    # what Tailscale itself answers
```

If Tailscale answers and the device does not, the device is not using Tailscale DNS (see
above). If neither answers, the name is wrong or MagicDNS is off: `tailscale status` shows
every machine's exact name.

**The tailnet was renamed.** The tailnet's name is the middle part of every MagicDNS name,
so all of them change with it, certificate included. dux follows on its own, but bookmarks
and codes you scanned earlier point at the old name: scan again. If the old answer lingers
in a DNS cache, flush it (`resolvectl flush-caches` on Linux with systemd-resolved).

**A `403` reading *"this dux server does not serve the requested host"*.** The mode is
`"no"`, the name belongs to another machine, dux has not read the name yet (on `"auto"` it
does within seconds of starting), the last look at Tailscale failed, or a Tailscale Funnel
route is on (see the caveats below).

**Every page answers `503`.** The page says which of three things it is. *Checking*: dux has
just started and its first look at Tailscale has not answered yet; wait a moment. *Could not
confirm*: dux could not ask Tailscale whether a Funnel publishes it; see
[When Tailscale isn't there](#when-tailscale-isnt-there) for every cause and the way out.
*A Funnel is publishing dux*: turn it off (see the caveats below). A browser tab that was
already open loses its connection the moment any of these starts, and reconnects once dux
serves again.

**The first HTTPS visit hangs.** Give it about 30 seconds: that is the certificate being
issued, once.

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
> If those tradeoffs bother you, put Tailscale's own HTTPS in front of dux:
> [one command, and dux picks it up by itself](#https-with-tailscale-serve).

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
> **dux is not meant to be exposed through Tailscale Funnel.** Funnel publishes a service to
> the anonymous public internet, and dux has no login. What dux does about it:
>
> - **It refuses every request that Tailscale marks as having come through Funnel**, on every
>   mode. Tailscale 1.72 and later mark them.
> - **While a Funnel on this machine forwards to dux's port, it refuses every request** on
>   every address, with a page saying why, until that Funnel is gone. That covers raw
>   connections and older Tailscale versions, which carry no mark and can claim to be
>   `localhost`. It also stops answering to this machine's name while any Funnel is on.
> - **It refuses everything until it has checked, and whenever it cannot check.** Unless the
>   mode is `"no"`, every request answers "checking" until dux's first look at Tailscale
>   lands, a moment after it starts. Whenever dux cannot confirm that no Funnel publishes it,
>   it refuses everything until it can. It serves without a successful look only when nothing
>   of Tailscale is on this machine; see
>   [When Tailscale isn't there](#when-tailscale-isnt-there) for exactly when, and the way
>   out.
> - **A Funnel already seen stays refused** until a look shows it gone, through a daemon
>   that stops or a command that fails.
> - **Pages already open are cut off.** The moment dux starts refusing, every browser tab
>   connected to it loses its live connection, terminals included, so a tab opened earlier
>   cannot keep typing into them.
>
> Each of these says so in a warning, and a failed check never lifts a refusal. Turn the Funnel off
> (`tailscale funnel status` lists it) and dux goes back to normal by itself.

> [!WARNING]
> Know the limits. dux looks every few seconds, so a Funnel switched on while dux runs is
> refused at the next look; with Tailscale 1.72 or later, a web Funnel is refused at once by
> its mark. dux only sees **this machine's** Funnels: another machine on your tailnet
> funnelling to this one's Tailscale address is invisible to it. On `"no"` dux does not
> consult Tailscale at all, so only the mark is checked, and switching to `"no"` lifts a
> refusal, with a warning that says so.

## Where to go next

- [Server mode overview](/docs/server-mode): the `[server]` keys in full, the startup banner,
  graceful shutdown, and the trust model.
- [The workspace in the browser](/docs/web-workspace): what you get once you are in,
  including the phone layout.
- [Hosting dux behind a login](/docs/public-hosting): the other answer, for when the machine
  is not on a private network.
