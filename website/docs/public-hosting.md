---
title: Hosting dux behind a login
description: A reverse proxy plus oauth2-proxy plus dux in one Docker Compose file, with GitHub sign-in restricted to an org, a team, or named accounts, and what breaks when a piece is missing.
group: Web UI
order: 66
---

> [!CAUTION]
> **There is no authentication layer in dux at all.** No password, no token, no
> accounts, and nothing that can be turned on in config. Anyone who can open the URL
> gets the whole workspace: every agent, every terminal, a shell on the machine through
> those terminals, git, and the server's filesystem through the project picker. This is
> [the trust model](/docs/server-mode#the-trust-model-stated-plainly), and everything
> below exists to compensate for it.

So the login has to live in front of dux. This page is one way to build that: a TLS
terminator, `oauth2-proxy` doing GitHub sign-in restricted to your org or to accounts you
name, and dux on a private network where nothing but the proxy can see it.

> [!TIP]
> If you do not actually need the public internet, you do not need any of this.
> [Tailscale](/docs/tailscale) puts the same workspace on a private network with
> nothing to publish and nothing to configure, and it is what the maintainer uses.

> [!IMPORTANT]
> Nothing about the Host allowlist or the same-origin check is access control. They
> stop a hostile web page tricking *your* browser into driving your server, which is a
> real attack and worth defending, and they do nothing at all about a person who simply
> visits the URL. If you read "dux has two automatic defenses" and relaxed, unrelax.

## The shape of it

Three containers on one private Compose network, and exactly one of them has a
published port.

```dot Only Caddy publishes a port. dux publishes none, so the proxy container is the only thing on the machine that can reach it.
digraph topology {
  bgcolor="transparent";
  rankdir=TB;
  nodesep=0.35;
  ranksep=0.45;
  node [shape=box, style="rounded,filled", fontname="Helvetica", fontsize=13, margin="0.30,0.18", penwidth=1];
  edge [fontname="Helvetica", fontsize=10, arrowsize=0.7, penwidth=1];

  internet [class="d-outside", label=<the internet<br/><font point-size="9">anyone at all</font>>];

  subgraph cluster_compose {
    label="one private Compose network";
    fontname="Helvetica";
    fontsize=10;
    labeljust="l";
    style="rounded";
    margin=16;

    caddy  [class="d-tls",  label=<caddy<br/><font point-size="9">:80 and :443, published</font>>];
    gate   [class="d-gate", label=<oauth2&#45;proxy<br/><font point-size="9">:4180, not published</font>>];
    duxsvc [class="d-app",  label=<dux server<br/><font point-size="9">:8080, never published</font>>];

    caddy -> gate   [label="  http"];
    gate  -> duxsvc [label="  http"];
  }

  internet -> caddy [label="  https"];
}
```

The order matters. TLS is outermost so the login cookie never crosses the internet in the
clear, and `oauth2-proxy` sits between Caddy and dux rather than beside it, so nothing
reaches dux without passing the gate.

## Before you start

You need three things.

**A hostname with DNS pointing at the box.** Caddy gets a certificate for it
automatically, and the name has to resolve first. Everything below uses `dux.example.com`;
replace it everywhere, including in the dux config.

**A GitHub OAuth app.** Create one under your account or your org
(`Settings → Developer settings → OAuth Apps`). The **Authorization callback URL** must
be exactly:

```text
https://dux.example.com/oauth2/callback
```

Keep the client ID and secret. To restrict by organization, create the app under a
personal account: the org check reads the GitHub API on the signed-in user's behalf, not as
an app installation.

**A cookie secret.** `oauth2-proxy` derives an AES key from it, and it validates at
startup that the value decodes to exactly 16, 24, or 32 bytes. Anything else aborts the
process with `cookie_secret must be 16, 24, or 32 bytes to create an AES cipher, but is
N bytes`:

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
    # Pin it. The upstream publishes a floating `latest`, and the thing standing
    # between the internet and your shell is not where you want a surprise.
    image: quay.io/oauth2-proxy/oauth2-proxy:v7.15.3
    restart: unless-stopped
    # No ports. Caddy reaches it over the private network by name.
    command:
      - --provider=github
      # Required. The default is 127.0.0.1:4180, which inside a container means
      # Caddy cannot reach it and every request fails at connect.
      - --http-address=0.0.0.0:4180
      # Docker's embedded DNS resolves `dux` to the service of that name on the
      # shared network. Resolution happens per request, so no depends_on is needed.
      - --upstream=http://dux:8080
      - --redirect-url=https://dux.example.com/oauth2/callback
      # Pick at least one gate. --github-user is an OR escape hatch: a listed
      # username is let in whether or not the org check would have passed.
      - --github-org=your-org
      # - --github-team=your-org:the-team
      # - --github-user=alice,bob
      #
      # oauth2-proxy runs its own email-domain check on top of the provider's
      # gate, and with nothing set the allowlist is empty and every login is
      # refused. "*" means "the GitHub gate above is the gate".
      - --email-domain=*
      # Accept Caddy's X-Forwarded-* headers, so the real client IP reaches the
      # logs and the post-login destination is chosen from the forwarded host
      # rather than the internal one. Defaults to false.
      - --reverse-proxy=true
      # With --reverse-proxy on, oauth2-proxy trusts those headers from ANY
      # source unless you say otherwise. Name the Compose network's subnet
      # (`docker network inspect` prints it); a wrong value costs you the real
      # client IP in the logs and nothing else.
      - --trusted-proxy-ip=172.16.0.0/12
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
      # Your repositories. dux does NOT create worktrees next to them: they go
      # under its own config directory, so they live on the dux_config volume
      # above. Do not treat that volume as disposable.
      - ./code:/root/code

volumes:
  caddy_data:
  caddy_config:
  dux_config:
```

Two flags are deliberately **not** in that list:

- **No `--scope`.** The GitHub provider's default is already `user:email read:org`, and
  `read:org` is what makes the org and team lookups work. Setting `--scope` replaces that
  default wholesale, which is the usual way an org check breaks.
- **No `--cookie-secure=false`.** It defaults to `true`, which is correct here.

> [!NOTE]
> `--email-domain=*` is safe in the YAML list above because Compose runs the entrypoint
> without a shell. Typing the same flag into a `docker run` at a prompt lets the shell
> glob the `*` against your working directory first; quote it.

The `Caddyfile` is three lines:

```caddyfile
dux.example.com {
	reverse_proxy oauth2-proxy:4180
}
```

That is the whole configuration because Caddy already does the two things dux needs with
no directives: it passes incoming headers through unchanged, `Host` included, adding only
`X-Forwarded-For`, `X-Forwarded-Proto` and `X-Forwarded-Host`, and it tunnels WebSockets.

One exception if you copy this into a different topology: since Caddy v2.11.0 an upstream
written as `https://` gets its `Host` overwritten to `{upstream_hostport}`. The upstream
here is plain `http://`, so it does not apply, but a variant that re-encrypts the hop has
to put the original back with `header_up Host {host}`.

And `.env`:

```bash
GITHUB_CLIENT_ID=Ov23li...
GITHUB_CLIENT_SECRET=...
COOKIE_SECRET=...   # the openssl output from above
```

### The one line of dux config

dux's Host allowlist accepts `localhost`, loopback addresses, and IP literals it actually
bound. `dux.example.com` is none of those, so every request gets a `403` until you say the
name out loud once:

```toml
[server]
allowed_hosts = ["dux.example.com"]
```

Hostnames only, no scheme and no port; the port is ignored when matching, matching is
case-insensitive, and a trailing dot is stripped. There is no wildcard, so `"*"` is a
literal hostname that matches nothing.

> [!IMPORTANT]
> Binding `0.0.0.0` does **not** get you out of this. An unspecified bind relaxes the
> allowlist for anything that parses as an IP address, and a hostname still is not one.

### Building the dux image

dux ships no image, so this part is yours. The container is not really "dux": it is your
whole development environment, the agent CLIs and their credentials, `git`, your git
identity, and whatever your projects need to build. A sketch:

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

# Then your agent CLIs, installed the way each project documents. Claude Code,
# Codex, OpenCode and Copilot ship as npm packages today.

WORKDIR /root/code
```

Then `docker compose exec dux gh auth login` and the equivalent for each agent CLI,
once.

> [!WARNING]
> Those credentials do **not** land on the `dux_config` volume. `gh` writes
> `/root/.config/gh` and each agent CLI writes its own directory, none of which is
> mounted above. Add volumes for them (or mount all of `/root`) if you want them to
> survive a restart. This is the part that takes an afternoon, and it has nothing to do
> with dux.

If containerising your dev environment is not appealing, run `dux server` on the host and
delete the `dux` service. One gotcha: the proxy container cannot reach a host-loopback
bind, so dux has to bind an address the container can route to, and the "no published port"
protection goes with it. Firewall that port, and remember the firewall is then the only
thing between the internet and a workspace with no login.

## What each piece is doing

**Caddy** terminates TLS. Without it you get the sign-in redirect loop below, and the
browser refuses dux a few APIs off a secure context: right-click paste, `OSC 52` clipboard
writes from an agent, desktop notifications, and PWA install.
[The Tailscale page lists the exact set](/docs/tailscale#plain-http-costs-you-a-few-browser-features),
and it is the same set here.

**`oauth2-proxy`** is the login. Without it, or reachable around it, you have published a
shell to the internet.

**The GitHub restriction flags** are what make the login mean something, since
`--provider=github` on its own authenticates *any* GitHub account. Set at least one:

- `--github-org=your-org` requires membership of that organization.
- `--github-team=your-org:the-team` requires that team. Combined with `--github-org`, both
  must match.
- `--github-user=alice,bob` is checked first and short-circuits: a listed username is
  admitted whether or not the org or team check would have passed. Set on its own, it is a
  complete allowlist, and an unlisted account is rejected outright.

> [!CAUTION]
> **The absent `ports:` entry on the `dux` service** is doing more work than any flag here.
> It makes "you cannot get to dux without passing the proxy" a property of the network
> rather than a promise. Add a published port for a quick test and you have removed the
> login for as long as it is there.

## What this does not give you

> [!CAUTION]
> Everyone through the door shares one workspace, because dux has no concept of a user to
> scope anything to. They can attach to any agent, type into a session someone else is
> mid-sentence in, run git, and browse the server's filesystem. There is no per-user
> ownership and no path sandbox, by design.
>
> So restrict the gate to people you would hand a terminal on that machine. An org-wide
> `--github-org` on a company with a thousand engineers is a thousand people with a shell.

**dux reads no `X-Forwarded-*` headers at all**, even though Caddy sets three and
`oauth2-proxy` adds `X-Forwarded-User` and `X-Forwarded-Email` on top. So dux's access log
carries no client identity: it records only the timestamp, method, path, status and
latency. Your audit trail lives in the proxy's logs, not in dux's.

> [!CAUTION]
> Do not use Tailscale Funnel, `ngrok`, `cloudflared` in its no-authentication mode, or
> anything else that publishes the port to the anonymous internet, as a shortcut around
> this page. The point of every paragraph above is that something has to ask who you are
> before dux answers. A tunnel that skips that step has not made this easier, it has
> made it public.

## When it does not work

Each of these fails in a distinctive way.

### The page loads, looks perfect, and nothing is ever live

Suspect the WebSocket upgrade. A proxy that does not pass upgrades through gives you a UI
that renders its first load correctly and then sits there: no terminal output, no status
changes, and eventually the in-app *Reconnecting…* overlay. Nothing returns a visible
error, which is why this one costs the most time.

Caddy needs no configuration for this, and `oauth2-proxy` proxies WebSockets by default
(`--proxy-websockets` is `true`). nginx does **not**, and needs it spelled out:

```nginx
proxy_http_version 1.1;
proxy_set_header Upgrade    $http_upgrade;
proxy_set_header Connection "upgrade";
proxy_read_timeout 1h;      # or an idle agent's socket is closed under you
```

### `403 this dux server does not serve the requested host`

The Host allowlist. The proxy is forwarding a `Host` that dux does not accept, almost
always your public hostname before you added it to `allowed_hosts`. Add it and reload dux's
config.

### `403 cross-origin WebSocket upgrade rejected`, or `cross-origin request rejected` on a write

The same-origin check. dux compares the browser's `Origin` against the request's `Host` on
every socket upgrade and every `POST`, `PATCH`, `PUT` and `DELETE`. A proxy that
**rewrites** `Host` to its backend target satisfies the allowlist, because loopback is
always allowed, and then fails this check on everything that matters, because the browser
still sends `dux.example.com` as `Origin`.

```dot Every hop forwards the same Host, and dux checks it against the browser's Origin. Rewrite Host anywhere in that chain and only the writes break.
digraph headers {
  bgcolor="transparent";
  rankdir=TB;
  nodesep=0.3;
  ranksep=0.5;
  node [shape=box, style="rounded,filled", fontname="Helvetica", fontsize=13, margin="0.30,0.18", penwidth=1];
  edge [fontname="Helvetica", fontsize=9, arrowsize=0.7, penwidth=1];

  browser [class="d-outside", label=<the browser<br/><font point-size="9">sends Origin, cannot be told not to</font>>];
  caddy   [class="d-tls",     label=<caddy<br/><font point-size="9">passes Host through by default</font>>];
  gate    [class="d-gate",    label=<oauth2&#45;proxy<br/><font point-size="9">pass&#45;host&#45;header defaults to true</font>>];
  duxsvc  [class="d-app",     label=<dux<br/><font point-size="9">Origin authority must equal Host</font>>];

  browser -> caddy  [label="  Host: dux.example.com"];
  caddy   -> gate   [label="  Host: dux.example.com"];
  gate    -> duxsvc [label="  Host: dux.example.com"];
}
```

The symptom is worse than a straight failure: the page loads, reads fine, and every single
action fails. The fix is to forward the original `Host`; adding the rewritten one to
`allowed_hosts` only silences the first check. Both hops above forward it unchanged out of
the box (`oauth2-proxy`'s `--pass-host-header` defaults to `true`); the two known ways to
lose it are nginx's `proxy_pass`, which rewrites `Host` to the upstream name unless you add
`proxy_set_header Host $host;`, and Caddy with an `https://` upstream.

The comparison is authority-only, so `http` versus `https` on the same host is not a
mismatch. A request with **no** `Origin` header passes deliberately, which keeps `curl` and
scripts working; `Origin: null` is treated as a mismatch and rejected.

### An endless redirect loop at sign-in

The session cookie. Either you are on plain HTTP with `--cookie-secure` at its `true`
default, so `oauth2-proxy` marks the cookie `Secure` and the browser refuses to store it,
or the `--redirect-url` does not exactly match the callback URL registered on the GitHub
OAuth app, down to the scheme and the `/oauth2/callback` path.

### GitHub sign-in succeeds and then you are denied

Two candidates. `--email-domain` is unset, so the domain allowlist is empty and rejects
everybody. Or `--scope` was set by hand without `read:org`, so the org membership lookup
comes back empty and `--github-org` matches nobody. Both look identical from the browser,
so check the `oauth2-proxy` logs, which name the org it wanted and the orgs it saw.

### The console shows one client IP for everybody

dux's per-request access log records no IP at all. Its connect and disconnect lines do
print a peer address, and since dux reads no forwarded headers that address is the proxy's
for every client. Use the proxy's logs to know who did something.

## Where to go next

- [Server mode overview](/docs/server-mode): the trust model in full, every `[server]`
  key with its default, the startup banner, and graceful shutdown.
- [Reaching dux over Tailscale](/docs/tailscale): the private-network answer, which
  needs none of this page and is what the maintainer actually uses.
- [The workspace in the browser](/docs/web-workspace): what you get once you are in,
  including the phone shell.
