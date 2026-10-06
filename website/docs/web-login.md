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

With dux running, the last lines instead say that dux reloaded, and that the new password is
in force and every signed-in browser is signed out.

![A terminal running dux config set server.auth.password: the hidden prompt with its strength meter reading weak > fair > good > strong > EXCELLENT, the second prompt, and the lines saying the hash is stored in /home/you/.config/dux/config.toml and that the change applies the next time dux starts.](/screens/config-set-password.png)

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
is running, `dux config set` asks it to reload, waits for the answer, and the password is in
force as soon as it has reloaded.

You can also set the first password from the browser, in **Preferences…** under
**Signing in**, but only from the machine dux runs on or from your tailnet. A browser
anywhere else is told to use `dux config set`, so a stranger who reaches an open dux cannot
set a password and lock you out of your own workspace. While dux
[cannot tell](#when-dux-cannot-tell-it-asks) whether a connection to `localhost` really is
this machine, the browser there is told the same, and why.

![The Password row at the top of Preferences, under Signing in: the current password field, a new password typed into the next field with a five-step strength meter reading Excellent under it, and the field to type it again.](/screens/preferences-password.png)

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
`P@ssw0rd!` rates only fair, below the default minimum, and four uncommon words strung
together rate excellent.

`dux config set` and Preferences refuse a password below either minimum and say why. The
browser's meter is a guide; dux itself makes the call when you save.

A hash pasted into `config.toml` by hand is checked for its format and cost, but dux cannot
measure a password it has never seen, so it measures it against the minimums each time
somebody signs in with it. A password below today's minimums still signs you in, and from
then on every browser using dux shows a banner saying the password is weaker than dux asks
for, with a way to change it. dux says the same once in its log and on the terminal UI's
status line. dux remembers this only while it runs: after a restart the banner stays away
until the next sign-in finds the password short again.

![The banner across the top of the web UI saying the dux password is weaker than the minimum it asks for, with Dismiss and Change it in Preferences… buttons.](/screens/weak-password-banner.png)

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
dux cannot vouch for them either. A machine where Tailscale is not installed, or is
installed but not running (the `tailscale` command says its daemon is stopped, and nothing
else of Tailscale is running), is an answer rather than a doubt: nothing there can publish
dux, so `localhost` is this machine again.

What dux announces, and where:

- **`tailscale = "no"` from the start**: one warning at start, in `dux.log` and in
  `dux server`'s output or the flip's log viewer. Switching to `"no"` while dux serves
  raises a warning on the terminal UI's status line and in the browser too. Switching back
  away from `"no"` says nothing: the next look at Tailscale decides, quietly when it finds
  nothing.
- **A Funnel, or a `tailscale` command dux cannot use**: a warning when it starts and a
  note when dux can vouch for `localhost` again, in its log, in the console, on the
  terminal UI's status line and in the browser.
- **A raw TCP forward**: a warning in the same places when it appears. When it goes away
  the warning is withdrawn from the status line and the browser, with no message of its
  own.
- **The moment after start**: nothing.

> [!WARNING]
> dux in a container cannot see a Tailscale outside it, so when it sees none it treats
> `localhost` as this machine and says once at start that it cannot tell. If anything
> outside could relay connections onto dux's port, set `require = "everywhere"`.

## Signing in, and staying signed in

A browser that needs the password gets the sign-in page in dux's own look. Sign in and you
land back on the exact page you asked for, the same agent and the same tab, because the
address in the URL is kept across the sign-in.

![The sign-in page: the dux logo, Sign in to dux, a line saying this dux asks for a password, a password field and the Sign in button.](/screens/login-trusted.png)

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

![The cog menu open under the Settings button, with Sign out as its last item.](/screens/app-menu-sign-out.png)

### Signing in from the command line

The `dux` [command line](/docs/command-line) on another machine signs in with the same
password: `dux remote login <name>` asks for it without echoing it, or reads it from a pipe
with `--stdin`. Its wrong guesses count exactly like the sign-in page's, slow-downs and bans
included, and `require` decides whether it is asked at all, the same as for a browser on
that machine.

A command-line sign-in outlives a browser's: it stays good between commands until it goes
unused for `cli_token_idle_days` (30 by default), so daily use never asks again. It ends at
once on `dux remote logout`, or when the password changes. It is kept on the signing-in
machine in `remotes.toml`, in the config folder, readable only by you.

## Changing the password

From a terminal, run `dux config set server.auth.password` again. From the browser, open
**Preferences…**, fill in the current password and the new one twice under **Signing in**,
and save. Leaving the fields empty keeps the password you have.

Either way, every browser is signed out, the one you changed it from included, and signs
in again with the new password. Every command-line sign-in ends too, so each machine runs
`dux remote login` again. A wrong current password counts as a failed sign-in, the
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
loopback, or knows something publishes its port, and has no password, it says so: in red
in `dux server`'s output and in the start-web-server flip's log viewer, and as a warning,
in the theme's warning color, on the terminal UI's status line while it serves in the
background. The browser shows a red banner across the top saying anyone who can reach the
address can use dux.

The banner also shows to any browser dux does not count as this machine, even when nothing
is reachable beyond it: a browser on `localhost` while dux
[cannot tell](#when-dux-cannot-tell-it-asks) who it is (with `tailscale = "no"` and only a
loopback listener, say) sees it, while the terminal says nothing, because no listener
reaches past this machine.

![dux server's startup output while listening on 0.0.0.0:3890 with no password: yellow warnings about the non-loopback address, and a red line saying no password is set and dux is reachable beyond this machine, so anyone who can reach it controls your agents and terminals.](/screens/server-no-password-warning.png)
![The start-web-server flip's log viewer serving on loopback and a Tailscale address, with the same red line saying no password is set and dux is reachable beyond this machine.](/screens/tui-flip-no-password-warning.png)
![The red banner across the top of the web UI: No password, anyone who can reach this address can use dux, with Dismiss and Don't show again buttons.](/screens/no-password-banner.png)

The banner has two buttons. **Dismiss** hides it for this page load only; it is meant to
come back. **Don't show again** sets `disable_no_auth_warning = true` in `config.toml` and
hides it everywhere for good; set it back to `false` to bring the banner back. The terminal
warnings do not go away: they are about the listener, and the listener is still there.

**The connection is not encrypted.** The sign-in page warns in red when you reached dux
over plain HTTP, because anyone on the network between you and dux can read the password as
you type it, take the session cookie after you sign in, and change the page before it
reaches you. dux leaves the warning off only where it knows the path is encrypted or never
leaves the machine: this machine, a direct tailnet connection (Tailscale encrypts it end to
end), `tailscale serve`, and Tailscale Funnel, which always serves HTTPS. The first three
count only while dux can tell who they are: while it
[cannot](#when-dux-cannot-tell-it-asks), `localhost` (on `tailscale = "no"` too), your
tailnet devices and `tailscale serve` visitors all see the warning, and a `tailscale serve`
HTTPS visitor's cookie is no longer marked Secure under `cookie_secure = "auto"`. (Under
`"auto"`, a cookie on `localhost` or a direct tailnet connection is never marked Secure:
those are plain HTTP, however private.)

![The sign-in page with a red box reading This connection is not encrypted, explaining that anyone on the network can read the password, take the session cookie and change the page.](/screens/login-plain-http.png)

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
  (30) in a minute, every visitor in that group is told to wait until the minute is over,
  including addresses that never failed, which stops a guesser that keeps changing
  address. The internet and proxies dux cannot vouch for share one count, and devices on
  your network have another, so a flood from one never locks out the other. A browser on
  this machine that opens dux on one of the machine's own non-loopback addresses has a
  count of its own. Your tailnet devices and this machine on `localhost` are slowed one
  address at a time while dux can tell who they are. While it
  [cannot](#when-dux-cannot-tell-it-asks), sign-ins on `localhost` share one count of their
  own, direct tailnet devices share the network's, and `tailscale serve` visitors share the
  internet's.
- **A block.** After `max_failed_logins` failures (5) from one address, each within
  `failed_login_window_seconds` (15 minutes) of the one before, dux adds that address to
  `blocked_addresses` in your `config.toml`. The count starts over only once that long has
  passed since the address's last failure. Its log, the terminal UI's status line and the browser all name the
  address and the file it was written to.

A blocked address gets a page saying it is blocked and where the block lives, and nothing
else: no sign-in page, no app, with or without a password.

![The page a blocked address gets when it opens dux: the dux duck over the heading This address is blocked, and a paragraph saying the block is an entry in blocked_addresses in the [server.auth] section of dux's config.toml, that whoever runs dux can remove it and reload, and that dux adds an address on its own after too many failed sign-ins.](/screens/blocked-page.png)

Which address a block lands on depends on what dux can check for itself:

- **Only an address dux can verify is ever written**: a device connected to dux directly,
  or one that came through your own `tailscale serve`. An address a request merely claims,
  through a reverse proxy or a Tailscale Funnel, is slowed down in memory and never
  written, since the client may have chosen it. dux's log names it, so you can add it by
  hand if you trust the proxy.
- **An IPv6 device is slowed per /64**, because one device can send from any address in
  its /64. When the addresses of one /64 together reach `max_failed_logins` without any
  one of them getting there alone, dux writes the whole /64 as one range, such as `"2001:db8:1:2::/64"`, and the single addresses of that
  /64 already in the list are folded into it, their comments kept. An address that
  reaches the limit on its own is blocked as itself.
- **Never grouped into a /64**, so slowed and blocked one address at a time: this machine's
  own network (every device on your LAN shares its /64), link-local `fe80::` addresses,
  your tailnet and Tailscale's address ranges, and IPv4. Each tailnet device is counted on
  its own.
- **Nothing already covered is written again.** An address inside a range the list already
  holds stays out of it.

**This machine is never blocked**, only slowed down, so you cannot lock yourself out from
the keyboard. The list never applies to loopback or to any of this machine's own
addresses: the part of an entry covering one is accepted, warned about and ignored, and
the rest of a range still applies.

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
`config.toml`, the list travels with your dotfiles. Once the list holds
`max_blocked_addresses` entries (1000), your own included, a new automatic block holds only
until dux restarts, and dux says so; entries you add yourself are never refused.

> [!CAUTION]
> **A block is per address, and an address can be many people.** Everyone behind one
> shared address (an office network, a phone carrier's NAT, a VPN exit) shares the
> slow-down and the block. Five wrong guesses by anyone there locks out everyone there,
> you included if you are one of them. Set `max_failed_logins = 0` if that is a worse risk
> for you than guessing: it turns blocks off, and the slow-down still applies.

## Behind a reverse proxy

dux classifies a request by the connection it arrived on, and believes forwarding headers
only from a proxy on the same machine:

- **A proxy on this machine that forwards to loopback and adds `X-Forwarded-For`** (Caddy
  pointing at `127.0.0.1:3890` does by default; nginx's `proxy_pass` does not, and needs
  `proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;`): dux counts its
  requests as everyone else, so the password applies under the default `require`. Failed
  sign-ins slow down the visitor's address as your proxy reports it, and all proxied
  traffic together, but dux never writes that address to `blocked_addresses`: it cannot
  check it. Add it by hand when its log names one you want gone.
- **A proxy that adds no forwarding header**, a default nginx included, makes every
  visitor look exactly like this machine, which needs no password under `"network"` or
  `"tailnet"`. dux cannot tell the difference.

> [!WARNING]
> A proxy on this machine that sends no forwarding header (`X-Forwarded-For`, `X-Real-IP`
> or `Forwarded`) lets every visitor in as this machine, with no password under the
> default `require`. Set `require = "everywhere"`, or make the proxy send one (for nginx,
> the line above); better, do both.

So behind a reverse proxy, use `"everywhere"`. dux warns once, on the first forwarded
request it sees while `require` is anything else; a proxy that sends no forwarding header
never triggers that warning.

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
as your tailnet, the sign-in cookie is marked Secure on its own, and the sign-in page shows
no plain-HTTP warning. All three hold only while dux can tell what reaches its port: while
it [cannot](#when-dux-cannot-tell-it-asks), those requests are everyone else, asked for the
password, shown the warning, and given a cookie that is not Secure unless
`cookie_secure = "always"`. See [HTTPS with `tailscale serve`](/docs/tailscale#https-with-tailscale-serve).

**Tailscale Funnel** publishes a port to the whole internet. With a password set, dux serves
through it: every Funnel visitor gets the sign-in page whatever `require` says, and while
the Funnel stands, so does a browser on this machine. A visitor who does not know the
password sees the sign-in page and nothing more. Funnel always serves HTTPS, so its visitors get a Secure cookie and no plain-HTTP
warning, and their failed sign-ins are slowed down but never written to
`blocked_addresses`, because dux cannot check the address Tailscale names for them.

With no password, dux still serves, because what you publish is your call, and it is as
loud about it as it can be: the warning in every serving mode, the red banner in every
browser (unless its **Don't show again** turned it off), and this machine's MagicDNS name
withdrawn while any Funnel is on, so a browser
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

![The page saying sign-in is misconfigured: dux refuses every protected request until the [server.auth] section of config.toml is fixed, with a Try again button.](/screens/broken-config-page.png)

## Changing settings while dux runs

Every `[server.auth]` setting applies on a reload; none needs a restart. `dux config set`
asks a running dux to reload by itself and says whether the reload worked. After editing
`config.toml` by hand, reload with the terminal UI palette's `reload-config`, the browser's
**Reload config** in the cog menu under **Configuration**, or from a shell:

```bash
kill -USR1 "$(head -n 1 ~/.config/dux/dux.lock)"   # the first line is the running dux's PID
```

On macOS the lock file is `~/.dux/dux.lock`.

## When the page cannot reach dux

Before it shows anything, the page asks dux whether this browser needs to sign in. If dux
does not answer, it says so, with a button to try again, rather than guessing either way.

![The page saying Can't reach dux: dux did not answer in time, so the page cannot tell whether it needs a password yet, with a Retry button.](/screens/cant-reach-dux.png)

## Every setting

All of these live under `[server.auth]`, are documented inline in `config.toml`, and are
listed with their defaults in
[Configuration](/docs/configuration#the-web-password-serverauth).
