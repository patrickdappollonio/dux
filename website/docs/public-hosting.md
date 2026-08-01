---
title: Hosting dux behind a login
description: A reverse proxy plus oauth2-proxy plus dux, in one Docker Compose file, with GitHub as the identity provider restricted to an org or to named accounts, plus what each piece is doing, what breaks when one is missing, and which parts are verified against dux's code rather than tested end to end.
group: Web UI
order: 66
---

dux has no login. That is stated plainly in
[the trust model](/docs/server-mode#the-trust-model-stated-plainly) and it is worth
repeating at the top of this page, because everything below exists to compensate for
it: **there is no authentication layer in dux at all.** No password, no token, no
accounts, and nothing that can be turned on in config. Anyone who can open the URL
gets the whole workspace: every agent, every terminal, a shell on the machine
through those terminals, git, and the server's filesystem through the project
picker.

So if you want dux somewhere reachable, the login has to live in front of it. This
page is one way to build that: a TLS terminator, `oauth2-proxy` doing GitHub sign-in
restricted to your org or to a list of accounts you name, and dux on a private
network where nothing but the proxy can see it.

> [!IMPORTANT]
> Nothing about the Host allowlist or the same-origin check is access control. They
> stop a hostile web page tricking *your* browser into driving your server, which is
> a real attack and worth defending, and they do nothing at all about a person who
> simply visits the URL. If you read "dux has two automatic defenses" and relaxed,
> unrelax.

## What is verified here, and what is not

The dux half of this page is read straight out of dux's source. The proxy half is
not, so the two are labelled throughout.

**Verified against dux's code:** that there is no authentication of any kind; that
the Host allowlist reads only the `Host` header and answers `403` with a plain-text
body; that the same-origin check compares `Origin` against `Host` on every WebSocket
upgrade and every `POST`/`PATCH`/`PUT`/`DELETE`; that dux reads no `X-Forwarded-*`
header anywhere; that terminals are WebSocket-only; and every flag and config key
quoted below.

**Not verified:** the Compose file itself. Nobody has run this exact stack end to
end. The `oauth2-proxy` and Caddy behaviour it depends on comes from those projects'
own documentation, not from a run, and their defaults can change under you. Treat it
as a starting configuration you are going to test, not a recipe you can paste and
walk away from. The [diagnostics](#when-it-does-not-work) section exists because
that testing is where you will need it.

The honest reason for the split: the maintainer's own daily setup is
[Tailscale](/docs/tailscale), which needs none of this. If you can get away with a
private network, do that instead and close this page.

## The shape of it

Three containers on one private Compose network, and exactly one port published to
the world:

```text
  internet ──▶ caddy          :443   TLS, and nothing else
                 │
                 ▼            http, private network
               oauth2-proxy   :4180  are you allowed in?
                 │
                 ▼            http, private network
               dux            :8080  no port published, ever
```

The order matters. TLS is outermost because the login cookie must not cross the
internet in the clear. `oauth2-proxy` sits between the proxy and dux rather than
beside it, so there is no code path that reaches dux without passing the gate: dux's
port is never published, so the only thing on the machine that can talk to it is the
proxy container.

## Before you start

You need three things.

**A hostname with DNS pointing at the box.** Caddy gets a certificate for it
automatically, and the name has to resolve for that to work. Everything below uses
`dux.example.com`; replace it everywhere, including in the dux config.

**A GitHub OAuth app.** Create one under your account or your org
(`Settings → Developer settings → OAuth Apps`). The **Authorization callback URL**
must be exactly:

```text
https://dux.example.com/oauth2/callback
```

Keep the client ID and secret. If you are going to restrict by organization, create
the app under a personal account and grant it `read:org` (below) rather than
worrying about org app policies; the org check is a read, not an install.

**A cookie secret.** `oauth2-proxy` signs its session cookie with it, and it must be
exactly 16, 24, or 32 bytes:

```bash
openssl rand -base64 32 | tr -- '+/' '-_'
```

## The Compose file

Save this as `compose.yaml`, alongside a `.env` holding the three secrets and a
`Caddyfile`.

```yaml
services:
  caddy:
    image: caddy:2-alpine
    restart: unless-stopped
    # The only published ports in the whole file. 80 is here so Caddy can answer
    # the ACME HTTP challenge and redirect to HTTPS; nothing is served on it.
    ports:
      - "80:80"
      - "443:443"
    volumes:
      - ./Caddyfile:/etc/caddy/Caddyfile:ro
      - caddy_data:/data
      - caddy_config:/config

  oauth2-proxy:
    image: quay.io/oauth2-proxy/oauth2-proxy:latest
    restart: unless-stopped
    # No ports. Caddy reaches it over the private network by name.
    command:
      - --provider=github
      - --http-address=0.0.0.0:4180
      - --upstream=http://dux:8080
      - --redirect-url=https://dux.example.com/oauth2/callback
      # read:org is what makes the org and team checks possible. Without it the
      # GitHub API answers with an empty membership list and everyone is denied.
      - --scope=user:email read:org
      # Pick ONE of the two gates below. Both is fine too: a user passes if
      # either matches.
      - --github-org=your-org
      # - --github-user=alice,bob
      #
      # oauth2-proxy checks the email domain on top of the provider's own gate,
      # and with nothing set the list is empty and every login is refused. "*"
      # means "the GitHub gate above is the gate".
      - --email-domain=*
      # Trust Caddy's X-Forwarded-* headers. Without it oauth2-proxy builds its
      # own redirect from the request it sees, which is plain http, and the
      # sign-in round trip lands you on an http:// URL.
      - --reverse-proxy=true
      # Skip the "Sign in with GitHub" interstitial: there is only one provider,
      # so the button is a click that asks nothing.
      - --skip-provider-button=true
    environment:
      OAUTH2_PROXY_CLIENT_ID: ${GITHUB_CLIENT_ID}
      OAUTH2_PROXY_CLIENT_SECRET: ${GITHUB_CLIENT_SECRET}
      OAUTH2_PROXY_COOKIE_SECRET: ${COOKIE_SECRET}

  dux:
    # There is no official dux image. Build one (see below) or point this at
    # your own; the important part is what is NOT here, which is a ports entry.
    build: ./dux
    restart: unless-stopped
    # Binds every interface INSIDE the container. That is not an exposure:
    # with no published port, the container's only neighbours are the other two
    # services. dux prints a warning about the non-loopback bind on startup and
    # in this topology that warning is expected.
    command: ["dux", "server", "--bind", "0.0.0.0:8080", "--no-tailscale"]
    volumes:
      # Your config, your session database and your log. Keep it on a named
      # volume or a bind mount: this is where projects and agents live.
      - dux_config:/root/.config/dux
      # Your repositories. dux creates worktrees next to them.
      - ./code:/root/code

volumes:
  caddy_data:
  caddy_config:
  dux_config:
```

The `Caddyfile` is three lines, because Caddy passes the `Host` header through and
handles WebSocket upgrades with no configuration at all, which are exactly the two
things dux needs:

```caddyfile
dux.example.com {
	reverse_proxy oauth2-proxy:4180
}
```

And `.env`:

```bash
GITHUB_CLIENT_ID=Ov23li...
GITHUB_CLIENT_SECRET=...
COOKIE_SECRET=...   # the openssl output from above
```

### The one line of dux config

dux's Host allowlist accepts `localhost`, loopback addresses, and IP literals it
actually bound. `dux.example.com` is a hostname, so it is none of those, and every
request would get a `403` until you say the name out loud once:

```toml
[server]
allowed_hosts = ["dux.example.com"]
```

Hostnames only, no scheme and no port; the port is ignored when matching, matching
is case-insensitive, and a trailing dot is stripped. There is no wildcard, so `"*"`
is a literal hostname that matches nothing.

Note that binding `0.0.0.0` does **not** get you out of this. An unspecified bind
relaxes the allowlist for anything that parses as an IP address, and a hostname
still is not one.

### Building the dux image

dux ships no image, so this part of the recipe is yours. The awkward truth is that
the container is not really "dux", it is your whole development environment:
the agent CLIs, their credentials, `git`, your git identity, and whatever your
projects need to build. A sketch:

```dockerfile
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl git openssh-client gnupg \
 && rm -rf /var/lib/apt/lists/*

# dux itself.
RUN curl -sSfL https://getdux.app/install.sh | DUX_INSTALL_DIR=/usr/local/bin bash

# gh is OPTIONAL. Skip it and dux hides the "new agent from a GitHub PR" entry
# and stops syncing PR status; nothing else changes.
RUN curl -sSfL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
      -o /usr/share/keyrings/githubcli-archive-keyring.gpg \
 && echo "deb [signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] \
      https://cli.github.com/packages stable main" \
      > /etc/apt/sources.list.d/github-cli.list \
 && apt-get update && apt-get install -y gh && rm -rf /var/lib/apt/lists/*

# Then your agent CLIs, however they install. Claude Code, Codex, OpenCode and
# Copilot are npm packages today; check each one's own instructions rather than
# trusting a line in a docs page to stay current.

WORKDIR /root/code
```

Then `docker compose exec dux gh auth login` and the equivalent for each agent CLI,
once, so the credentials land on the `dux_config` volume and survive a restart. This
is the part that takes an afternoon, and it has nothing to do with dux.

If containerising your dev environment is not appealing, run `dux server` on the
host instead and delete the `dux` service. One gotcha if you do: the proxy container
cannot reach a host-loopback bind, so dux has to bind an address the container can
route to, and the whole "no published port" protection goes with it. Firewall the
port at that point, and remember the firewall is now the only thing standing between
the internet and a workspace with no login.

## What each piece is doing, and what happens without it

**Caddy** terminates TLS and does nothing else. Without it you are on plain HTTP,
and two things follow. The `oauth2-proxy` session cookie defaults to `Secure`, so
the browser will not store it and you get an infinite redirect loop at sign-in
rather than an error. And the browser refuses dux a few APIs off a secure context:
right-click paste, `OSC 52` clipboard writes from an agent, desktop notifications,
and PWA install.
[The Tailscale page lists the exact set](/docs/tailscale#plain-http-costs-you-a-few-browser-features),
and it is the same set here.

**`oauth2-proxy`** is the login. Without it, or reachable around it, you have
published a shell to the internet. That is not hyperbole and it is not a
misconfiguration risk you can mitigate with care: a dux terminal is a terminal.

**`--github-org` or `--github-user`** is what makes the login mean something.
`--provider=github` on its own authenticates *any* GitHub account, which is
approximately everyone. `--github-org=your-org` restricts to members of an
organization; `--github-user=alice,bob` allows the accounts you name whether or not
they are in it. Set at least one.

**`--email-domain=*`** is the counterintuitive one. `oauth2-proxy` applies its own
email-domain filter on top of the provider's gate, and an unset list is an empty
allowlist, so every login is refused after a perfectly successful GitHub sign-in.
`*` says the GitHub gate is the gate.

**The absent `ports:` entry on the `dux` service** is doing more work than any flag
here. It is what makes "you cannot get to dux without passing the proxy" a property
of the network rather than a promise. If you add a published port for a quick test,
you have removed the login for as long as it is there.

## What this does not give you

`oauth2-proxy` is a door, not a permission system. Once someone is through it they
are inside the same single workspace as everyone else who is through it, because
dux has no concept of a user to scope anything to. They can attach to any agent,
type into a session someone else is mid-sentence in, run git, and browse the
server's filesystem through the project picker. There is no per-user ownership and
no path sandbox, by design.

So restrict the gate to people you would hand a terminal on that machine, not to
everyone in a large org. An org-wide `--github-org` on a company with a thousand
engineers is a thousand people with a shell.

One more thing that surprises people: **dux reads no `X-Forwarded-*` headers at
all.** Nothing in dux inspects `X-Forwarded-For`, `X-Forwarded-Proto` or
`X-Forwarded-Host`. Practically that means dux's access log records the proxy's IP
for every request, and it means the identity `oauth2-proxy` established is not
visible to dux and never will be. Your audit trail lives in the proxy's logs, not in
dux's.

> [!CAUTION]
> Do not use Tailscale Funnel, `ngrok`, `cloudflared` in its no-authentication mode,
> or anything else that publishes the port to the anonymous internet, as a shortcut
> around this page. The point of every paragraph above is that something has to ask
> who you are before dux answers. A tunnel that skips that step has not made this
> easier, it has made it public.

## When it does not work

Each of these fails in a distinctive way, which is the good news.

### The page loads, looks perfect, and nothing is ever live

Suspect the WebSocket upgrade. Terminals are WebSocket-only, and so is the change
feed that tells the page when anything moved, so a proxy that does not pass upgrades
through gives you a UI that renders its first load correctly and then sits there:
no terminal output, no status changes, and eventually the in-app *Reconnecting…*
overlay. Nothing returns a visible error, which is why this one costs the most time.

Caddy needs no configuration for this. `oauth2-proxy` proxies WebSockets by default
(`--proxy-websockets` is `true`). nginx does **not**, and needs it spelled out:

```nginx
proxy_http_version 1.1;
proxy_set_header Upgrade    $http_upgrade;
proxy_set_header Connection "upgrade";
proxy_read_timeout 1h;      # or an idle agent's socket is closed under you
```

### `403 this dux server does not serve the requested host`

The Host allowlist. The proxy is forwarding a `Host` that dux does not accept,
almost always your public hostname before you added it to `allowed_hosts`. Add it,
reload dux's config, done. The body of the `403` is that exact sentence, so you do
not have to guess which check fired.

### `403 cross-origin WebSocket upgrade rejected`, or `cross-origin request rejected` on a write

The same-origin check, and this is the trap worth reading twice. dux compares the
browser's `Origin` against the request's `Host` on every socket upgrade and every
`POST`, `PATCH`, `PUT` and `DELETE`. A proxy that **rewrites** `Host` to its backend
target satisfies the allowlist, because loopback is always allowed, and then fails
this check on everything that matters, because the browser is still sending
`dux.example.com` as `Origin`.

The symptom is worse than a straight failure: the page loads, reads fine, and every
single action fails. The fix is to forward the original `Host`, never to add the
rewritten one to `allowed_hosts`, which silences the first check and leaves the
second exactly as broken.

Caddy forwards the incoming `Host` by default, and `oauth2-proxy` does too
(`--pass-host-header` is `true`). Two known ways to lose it anyway: nginx's
`proxy_pass` rewrites `Host` to the upstream name unless you add
`proxy_set_header Host $host;`, and Caddy rewrites it when the upstream is `https://`
(a deliberate change in v2.11 for SNI). The upstream here is plain `http://` inside
a private network, so that second one does not bite this file, but it will bite a
variant of it.

The comparison is authority-only, so `http` versus `https` on the same host is not a
mismatch. A request with **no** `Origin` header at all passes deliberately, which is
what keeps `curl` and scripts working; a request with `Origin: null` is treated as a
mismatch and rejected.

### An endless redirect loop at sign-in

The session cookie. Either you are on plain HTTP with `--cookie-secure` at its
`true` default, or the `--redirect-url` does not exactly match the callback URL
registered on the GitHub OAuth app, down to the scheme and the `/oauth2/callback`
path.

### GitHub sign-in succeeds and then you are denied

Two candidates and they are both easy. `--email-domain` is unset, so the domain
allowlist is empty and rejects everybody. Or the `read:org` scope is missing, so the
org membership lookup comes back empty and `--github-org` matches nobody. Both look
identical from the browser, so check the `oauth2-proxy` logs, which say which one it
was.

### Everything works, but the access log is all one IP

Working as designed. dux reads no forwarded headers, so it logs the connecting
socket, which behind a proxy is always the proxy. Use the proxy's logs when you want
to know who did something.

## Where to go next

- [Server mode overview](/docs/server-mode): the trust model in full, every
  `[server]` key with its default, the startup banner, and graceful shutdown.
- [Reaching dux over Tailscale](/docs/tailscale): the private-network answer, which
  needs none of this page and is what the maintainer actually uses.
- [The workspace in the browser](/docs/web-workspace): what you get once you are in,
  including the phone shell.
