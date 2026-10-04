---
title: The web password
description: Put a password in front of the web UI, decide who is asked for it, and know what it does and does not protect against, from the first prompt to bans, proxies, Tailscale Funnel and plain HTTP.
group: Web UI
order: 60.5
---

The web UI can ask for a password. It is one password for one owner: there are no
accounts, and everyone who signs in gets the same single workspace as before. What changes
is who gets that far. With a password set, a visitor from somewhere dux does not trust sees
a sign-in page instead of your agents.

It is off until you set one. Without a password, anyone who can reach the port drives your
agents and terminals, and dux says so loudly whenever that reach goes beyond this machine.

> [!IMPORTANT]
> The password guards the web UI, not the machine. Once someone is signed in they have a
> terminal on the box dux runs on, with your user's rights, so treat signing in as handing
> over a shell.

## Setting the first password

From a terminal on the machine dux runs on:

```bash
dux config set server.auth.password
```

dux asks twice and never echoes what you type. While you type, the prompt rates the
password, and says when it falls short of the minimums:

```text
New web UI password [weak > fair > GOOD > strong > excellent]:
Type it again:
The web UI password is stored (strength: good): its Argon2id hash is in server.auth.password_hash in /home/you/.config/dux/config.toml, and the password itself is stored nowhere.
dux is not running, so the change applies the next time it starts.
```

With dux running, the last line instead says dux was asked to reload, and that once it has,
the new password is in force and every signed-in browser is signed out.

<!-- screenshot: config-set-password -->

For scripts, pipe it in instead:

```bash
printf '%s\n' "$DUX_PASSWORD" | dux config set server.auth.password --stdin
```

One trailing line break is dropped. A password cannot hold a line break, a tab or any other
control character, because a browser's password field cannot type one, so a pipe holding a
second line break is refused. `--stdin` refuses to read from a terminal, where what you
typed would show, and a run with no terminal to ask on tells you to use `--stdin`.

> [!NOTE]
> The password is never accepted as a command-line argument. Shell history and the process
> list would both keep it, so `dux config set server.auth.password hunter2` is refused and
> changes nothing.

What lands in `config.toml` is only the password's Argon2id hash, at
`server.auth.password_hash`. `dux config get server.auth.password` prints that hash. If dux
is running, `dux config set` asks it to reload, and the password is in force as soon as it
has.

You can also set the first password from the browser, in **Preferences…** under
**Signing in**, but only from the machine dux runs on or from your tailnet. A browser
anywhere else is told to use `dux config set`, so a stranger who reaches an open dux cannot
set a password and lock you out of your own workspace. While dux
[cannot tell](#when-dux-cannot-tell-it-asks) whether a connection to `localhost` really is
this machine, the browser there is told the same, and why.

<!-- screenshot: preferences-password -->

To remove the password again:

```bash
dux config set server.auth.password_hash ""
```

### How strong is strong enough

A new password has to be at least `minimum_password_length` characters (12 by default) and
reach `minimum_password_score` (2, "good", by default) on a scale from 0 (weak) to 4
(excellent). The score is an estimate of how many guesses the password would take, with
dictionary words, names, dates, keyboard runs, l33t spellings, `dux` and your user name all
counting against it. That is why there are no rules about symbols or capitals:
`P@ssw0rd!` rates weak, and four uncommon words strung together rate excellent.

`dux config set` and Preferences refuse a password below either minimum and say why. The
browser's meter is a guide; dux itself makes the call when you save.

A hash pasted into `config.toml` by hand is checked for its format and cost, but dux cannot
measure a password it has never seen, so it checks that one the first time somebody signs
in with it. A password below today's minimums still signs you in, and from then on every
browser using dux shows a banner saying the password is weaker than dux asks for, with a way to change
it. dux says the same once in its log and on the terminal UI's status line.

<!-- screenshot: weak-password-banner -->

## Who is asked for it

Once a password is set, `require` decides where it applies:

```toml
[server.auth]
require = "network"   # or "tailnet", or "everywhere"
```

| `require` | This machine | Your tailnet | Everyone else |
| --- | --- | --- | --- |
| `"network"` (default) | no password | no password | password |
| `"tailnet"` | no password | password | password |
| `"everywhere"` | password | password | password |

What each column means:

- **This machine** is a browser on the machine dux runs on, opening `localhost` or
  `127.0.0.1`, with nothing relaying the request. Opening dux on one of this machine's other
  addresses (its LAN address, or its own Tailscale address or MagicDNS name) is not this
  machine: a relay running on the machine arrives from those same addresses, so dux counts
  it as everyone else.
- **Your tailnet** is another device opening dux on this machine's Tailscale address or
  MagicDNS name, or through a `tailscale serve` route to dux. A request through
  `tailscale serve` counts only when Tailscale says which tailnet user sent it; one that
  arrives without that identity (from a tagged device, say) is treated as everyone else.
- **Everyone else** is your LAN, a reverse proxy dux cannot vouch for, and the public
  internet through Tailscale Funnel. Funnel traffic is always asked, whatever `require`
  says.

`require` has no effect while no password is set.

### When dux cannot tell, it asks

dux decides "this machine" from the connection itself, and something else on the machine
can hand it a connection that only looks local: a raw TCP forward onto dux's port makes
whoever comes through it look exactly like this machine. So whenever dux cannot rule that
out, a request to `localhost` counts as everyone else, and with a password set, you sign in
on this machine too, whatever `require` says. That happens:

- **the whole time with `tailscale = "no"`** (or `--no-tailscale`): dux does not consult
  Tailscale at all, so it can never rule a Funnel or a forward out. It says so once at
  start;
- for a moment after dux starts, until its first look at Tailscale answers;
- while dux cannot ask Tailscale whether anything publishes its port: the `tailscale`
  command fails or does not answer, or it is not on your `PATH`, in `/usr/local/bin` or in
  the macOS app while Tailscale is running on this machine;
- while a Tailscale Funnel, or a raw TCP forward (`tailscale serve --tcp`), points at
  dux's port.

In all of these but a Funnel, your tailnet devices are asked for the password too, since
dux cannot vouch for them either. A machine with no Tailscale at all (no `tailscale`
command and nothing of Tailscale running) is an answer rather than a doubt: nothing there
can publish dux, so `localhost` is this machine again.

Apart from the moment after start, dux says when this starts and when it stops, in its
log, on the terminal UI's status line and in the browser.

> [!WARNING]
> dux in a container cannot see a Tailscale outside it, so when it sees none it treats
> `localhost` as this machine and says once at start that it cannot tell. If anything
> outside could relay connections onto dux's port, set `require = "everywhere"`.

## Signing in, and staying signed in

A browser that needs the password gets the sign-in page in dux's own look. Sign in and you
land back on the exact page you asked for, the same agent and the same tab, because the
address in the URL is kept across the sign-in.

<!-- screenshot: login-trusted -->

A session lasts as long as you use it:

- **An open dux tab keeps you signed in.** Its live connection counts as activity, so a
  tab left open for hours waiting on an agent is still signed in when you come back, even
  in the background.
- **Close every tab** and the session ends `session_idle_seconds` later (60 by default).
- **A quick restart of dux keeps you signed in.** An open tab reloads itself after a
  restart, as it always has, and walks straight back in as long as the session had not
  already ended.

The sign-in cookie is named after dux's port, so two dux servers on one machine never sign
each other's browsers out. Page scripts cannot read it, and other sites cannot send it.

> [!WARNING]
> **Browsers do not keep cookies apart by port.** Any other web app you open under the same
> host name as dux (another port on `localhost`, on your MagicDNS name or on your own
> domain) is sent dux's session cookie with every request, and whoever runs that app can
> use it to act as you in dux for as long as the session lasts. Serve dux under a host name
> of its own, or only open apps you trust under the one it shares. This is a limit of how
> browsers handle cookies, and dux accepts it rather than working around it.

**Signing out** is **Sign out**, the last item in the cog menu. It appears only when there
is a session to end. Your place and any unsaved editor drafts stay in the page, so signing
back in picks up where you were. On a connection that needs no password, signing out
reopens dux at once, and a note says so.

<!-- screenshot: app-menu-sign-out -->

## Changing the password

From a terminal, run `dux config set server.auth.password` again. From the browser, open
**Preferences…**, fill in the current password and the new one twice under **Signing in**,
and save. Leaving the fields empty keeps the password you have.

Either way, every browser is signed out, the one you changed it from included, and signs
in again with the new password. A wrong current password counts as a failed sign-in, the
same as one on the sign-in page.

The raw config editor in the browser (**Configuration → Edit config file…**) refuses any
edit that sets, changes or removes the password, and writes nothing. That stops the
password being swapped from a page that never asked for the current one. For the same
reason it refuses a change to `[server] host` or `allowed_hosts`, including one made
through an older setting name that dux turns into one of them; make those in the file from
a terminal.

If `config.toml` changed on disk since the editor opened it (a `dux config set`, a new
block, another editor), the save is refused and nothing is written. The editor then offers
to reload the file, dropping your edits, or to keep editing so you can copy what you need
first.

## Warnings you will see

**No password, reachable beyond this machine.** When dux listens somewhere other than
loopback, or knows something publishes its port, and has no password, it says so in red:
in `dux server`'s output, in the start-web-server flip's log viewer, and on the terminal
UI's status line while it serves in the background. The browser shows a red banner across
the top saying anyone who can reach the address can use dux.

<!-- screenshot: server-no-password-warning -->
<!-- screenshot: tui-flip-no-password-warning -->
<!-- screenshot: no-password-banner -->

The banner has two buttons. **Dismiss** hides it for this page load only; it is meant to
come back. **Don't show again** sets `disable_no_auth_warning = true` in `config.toml` and
hides it everywhere for good; set it back to `false` to bring the banner back. The terminal
warnings do not go away: they are about the listener, and the listener is still there.

**The connection is not encrypted.** The sign-in page warns in red when you reached dux
over plain HTTP, because anyone on the network between you and dux can read the password as
you type it, take the session cookie after you sign in, and change the page before it
reaches you. dux leaves the warning off only where it knows the path is encrypted or never
leaves the machine: this machine, a direct tailnet connection (Tailscale encrypts it end to
end), `tailscale serve`, and Tailscale Funnel, which always serves HTTPS.

<!-- screenshot: login-plain-http -->

> [!NOTE]
> Behind your own HTTPS proxy, the browser's side of the connection is encrypted, but dux
> cannot see that from where it sits. Set `cookie_secure = "always"` there: it tells dux
> that browsers reach it over HTTPS, so the warning goes away (see
> [the cookie's Secure flag](#the-cookies-secure-flag)).

## Failed sign-ins and blocked addresses

A wrong password costs time, and repeated wrong passwords cost the address:

- **A slow-down per address.** After a failed sign-in, the next attempt from that address
  waits `failed_login_delay_seconds` (1 by default), doubling with each further failure up
  to `failed_login_max_delay_seconds` (30). The sign-in page counts the wait down.
- **A limit for many addresses together.** Past `max_failed_logins_per_minute` failures
  (30) in a minute, the addresses those failures came from are told to wait until the
  minute is over, which stops a guesser that keeps changing address. The internet and
  proxies dux cannot vouch for share one count, and devices on your network have another,
  so a flood from one never locks out the other. Your tailnet devices and this machine are
  only ever slowed one address at a time.
- **A block.** After `max_failed_logins` failures (5) from one address within
  `failed_login_window_seconds` (15 minutes), dux adds that address to `blocked_addresses`
  in your `config.toml`. Its log, the terminal UI's status line and the browser all name the
  address and the file it was written to.

A blocked address gets a page saying it is blocked and where the block lives, and nothing
else: no sign-in page, no app, with or without a password.

<!-- screenshot: blocked-page -->

Which address a block lands on depends on what dux can check for itself:

- **Only an address dux can verify is ever written**: a device connected to dux directly,
  or one that came through your own `tailscale serve`. An address a request merely claims,
  through a reverse proxy or a Tailscale Funnel, is slowed down in memory and never
  written, since the client may have chosen it. dux's log names it, so you can add it by
  hand if you trust the proxy.
- **An IPv6 device is slowed per /64**, because one device can send from any address in
  its /64. When the addresses of one /64 together reach `max_failed_logins`, dux writes the
  whole /64 as one range, such as `"2001:db8:1:2::/64"`, and the single addresses of that
  /64 already in the list are folded into it, their comments kept.
- **Never grouped into a /64**, so slowed and blocked one address at a time: this machine's
  own network (every device on your LAN shares its /64), link-local `fe80::` addresses,
  your tailnet and Tailscale's address ranges, and IPv4. Each tailnet device is counted on
  its own.
- **Nothing already covered is written again.** An address inside a range the list already
  holds stays out of it.

**This machine is never blocked**, only slowed down, so you cannot lock yourself out from
the keyboard. The list never applies to loopback or to any of this machine's own
addresses: an entry covering one is accepted, warned about and ignored.

> [!NOTE]
> Through a `tailscale serve` TCP forward, outsiders arrive looking exactly like this
> machine, so their failed sign-ins slow down your own sign-ins on `localhost` too. dux
> says so when it sees such a forward. Remove the forward to separate them.

To lift a block, remove the address from `blocked_addresses` and reload. The quickest way
is to write the list you want back, which reloads a running dux by itself:

```bash
dux config get server.auth.blocked_addresses
dux config set server.auth.blocked_addresses '["198.51.100.23"]'   # the list without yours
```

After a hand edit, reload as described [below](#changing-settings-while-dux-runs).

`blocked_addresses` is yours as much as dux's. Add a scanner you keep seeing in the access
log: entries can be single addresses (`"203.0.113.7"`, `"2001:db8::1"`) or ranges
(`"203.0.113.0/24"`), and they apply with or without a password. Since it lives in
`config.toml`, the list travels with your dotfiles. Once dux's own additions reach
`max_blocked_addresses` (1000), a new block holds only until dux restarts, and dux says so;
entries you add yourself are never refused.

> [!CAUTION]
> **A block is per address, and an address can be many people.** Everyone behind one
> shared address (an office network, a phone carrier's NAT, a VPN exit) shares the
> slow-down and the block. Five wrong guesses by anyone there locks out everyone there,
> you included if you are one of them. Set `max_failed_logins = 0` if that is a worse risk
> for you than guessing: it turns blocks off, and the slow-down still applies.

## Behind a reverse proxy

dux classifies a request by the connection it arrived on, and believes forwarding headers
only from a proxy on the same machine:

- **A proxy on this machine that forwards to loopback** (nginx or Caddy pointing at
  `127.0.0.1:3890`, adding `X-Forwarded-For` as both do by default): dux counts its
  requests as everyone else, so the password applies under the default `require`. Failed
  sign-ins slow down the visitor's address as your proxy reports it, and all proxied
  traffic together, but dux never writes that address to `blocked_addresses`: it cannot
  check it. Add it by hand when its log names one you want gone.
- **A proxy that adds no forwarding header** makes every visitor look exactly like this
  machine, which needs no password under `"network"` or `"tailnet"`. dux cannot tell the
  difference.

So behind a reverse proxy, use `"everywhere"`. dux warns once, on the first forwarded
request it sees while `require` is anything else.

```toml
[server]
allowed_hosts = ["dux.example.com"]

[server.auth]
require       = "everywhere"
cookie_secure = "always"   # the proxy speaks HTTPS to the browser; dux cannot see that
```

A proxy on another machine or in another container reaches dux over the network, so the
password applies whatever `require` says (unless it connects to dux's Tailscale address,
which makes it your tailnet). Every request then comes from the proxy's own
address, which means the slow-down and the block treat all your visitors as one, as in the
caution above. [Hosting dux on the public internet](/docs/public-hosting) walks through a
full setup.

## The cookie's Secure flag

A cookie marked Secure is never sent over plain HTTP. `cookie_secure` decides when dux
marks it:

- **`"auto"`** (the default): only when dux knows the browser reached it over HTTPS, which
  today means a `tailscale serve` HTTPS route or a Tailscale Funnel.
- **`"always"`**: always. Use this behind an HTTPS proxy of your own. It also tells dux that
  browsers reach it over HTTPS, so the sign-in page drops its plain-HTTP warning. A browser
  on plain HTTP then cannot keep the cookie, so it cannot stay signed in.
- **`"never"`**: never.

## Tailscale serve and Funnel

**`tailscale serve`** keeps dux on your tailnet with real HTTPS. Requests through it count
as your tailnet (unless dux [cannot tell](#when-dux-cannot-tell-it-asks) what reaches its
port), the sign-in cookie is marked Secure on its own, and the sign-in page shows no
plain-HTTP warning. See [HTTPS with `tailscale serve`](/docs/tailscale#https-with-tailscale-serve).

**Tailscale Funnel** publishes a port to the whole internet. With a password set, dux serves
through it: every Funnel visitor gets the sign-in page whatever `require` says, and while
the Funnel stands, so does a browser on this machine. A visitor who does not know the
password sees the sign-in page and nothing more. Funnel always serves HTTPS, so its visitors get a Secure cookie and no plain-HTTP
warning, and their failed sign-ins are slowed down but never written to
`blocked_addresses`, because dux cannot check the address Tailscale names for them.

With no password, dux still serves, because what you publish is your call, and it is as
loud about it as it can be: the red warning in every serving mode, the red banner in every
browser, and this machine's MagicDNS name withdrawn while any Funnel is on, so a browser
opening the Funnel's address gets a `403`. That last one is not a lock: a request crafted to
name another host gets through. Set a password before you Funnel dux.

> [!WARNING]
> dux only sees this machine's own Funnels. Another tailnet machine that relays the public
> internet to this one's Tailscale address arrives looking like your tailnet, and a program
> with Tailscale built into it that relays onto `localhost` looks like this machine. If
> either could be true for you, use `require = "tailnet"` or `"everywhere"`.

## What the password cannot do

> [!CAUTION]
> **A published hash can be guessed offline.** The hash is not your password, so
> `config.toml` can live in a dotfiles repository, but anyone holding the hash can try
> guesses on their own hardware as fast as it allows, and no ban or slow-down applies
> there. Use a long, unique, generated password that you use nowhere else, and keep the
> repository private if you can.

- **A stolen cookie works while it is used.** Somebody who copies a live session cookie
  out of your browser can use it, and as long as they keep using it, it does not expire.
  Changing the password signs every session out, theirs included.
- **Anyone with a shell can edit the config.** The password protects against someone
  reaching dux over the network, not against someone who can already run commands as you
  on that machine, which includes anyone signed in to dux. They can change `config.toml`,
  password and all.
- **Plain HTTP exposes it.** See the warning above. Use your tailnet, `tailscale serve`, or
  an HTTPS proxy.

## When `[server.auth]` is broken

Elsewhere in `config.toml`, a bad value can fall back to its default (`dux server` does
that for a value of the wrong type). This section never does, because its default is "no
password", and a typo must never quietly open dux to everyone. A misspelled key, a value of the wrong type or out of range,
an unreadable `password_hash`, a password setting written outside `[server.auth]` (or in a
table with a near-miss name such as `[server.auht]`), a plaintext `password` anywhere in
the file, or a `config.toml` that is not valid TOML at all:

- **stops dux from starting**, with a message naming the file, the line and the problem;
- **makes a reload change nothing**: the running dux keeps the settings it had, password
  included, and says why.

`dux config get` and `dux config set` keep working on a broken section, so you can look at
it and repair it one problem at a time; see
[Reading and changing one setting](/docs/configuration#reading-and-changing-one-setting).
Should a broken section ever reach a running dux anyway, the browser shows a page saying
sign-in is misconfigured and lets nobody in until the file is fixed.

<!-- screenshot: broken-config-page -->

## Changing settings while dux runs

Every `[server.auth]` setting applies on a reload; none needs a restart. `dux config set`
asks a running dux to reload by itself and says whether it could. After editing
`config.toml` by hand, reload with **Reload config** (in the terminal UI's palette, or the
browser's cog menu under **Configuration**), or from a shell:

```bash
kill -USR1 "$(head -n 1 ~/.config/dux/dux.lock)"   # the first line is the running dux's PID
```

On macOS the lock file is `~/.dux/dux.lock`.

> [!CAUTION]
> A dux older than this release stops instead of reloading when it gets that signal, and
> takes every agent and terminal with it. Send it only when `dux.lock` has a second line
> reading `reload-signal=usr1`; otherwise restart dux. `dux config set` checks this for
> you.

## When the page cannot reach dux

Before it shows anything, the page asks dux whether this browser needs to sign in. If dux
does not answer, it says so, with a button to try again, rather than guessing either way.

<!-- screenshot: cant-reach-dux -->

## Every setting

All of these live under `[server.auth]`, are documented inline in `config.toml`, and are
listed with their defaults in
[Configuration](/docs/configuration#the-web-password-serverauth).
