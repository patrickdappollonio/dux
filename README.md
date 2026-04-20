# dux

<img src="assets/dux-logo.png" width="200" align="right" />

Your AI agents deserve a proper office. **dux** (pronounced "dooks") is a terminal UI that lets you run multiple AI coding agents side by side, each in its own git worktree, with full companion terminals, macros, commit generation, and a command palette that knows more tricks than you do.

No protocol layers. No adapters. No JSON-RPC. Just real CLIs running in real terminals.

Oh, and it's fast and consumes low resources: more RAM is left for Claude, Codex or any of the other agents 👍

[![asciicast](assets/dux-screenshot.svg)](https://asciinema.org/a/IvqL89rXvwCzvSxQ)

## Why dux?

Most AI coding tools give you one agent in one directory. dux gives you **unlimited agents across unlimited worktrees**, all visible at once. Spawn five agents on five branches and let them work in parallel. Fork a session to try a different approach without losing the original. Open companion terminals next to your agents for builds, tests, or just poking around.

Every agent runs through a PTY, the same pseudo-terminal your shell uses. That means the CLI tool (Claude, Codex, Gemini, OpenCode, or literally anything else) runs exactly like it would in your regular terminal. Your MCP servers, hooks, skills, slash commands, and permission dialogs all work. We don't mess with your setup.

## Install

**Homebrew:**

```bash
brew install patrickdappollonio/tap/dux
```

**Shell:**

```bash
curl -sSfL https://github.com/patrickdappollonio/dux/releases/latest/download/install.sh | bash
```

By default, the script installs to `~/.local/bin` if it exists and is in your `PATH`, otherwise `/usr/local/bin`. You can override the install directory or pin a specific version:

```bash
# Custom install directory
curl -sSfL https://github.com/patrickdappollonio/dux/releases/latest/download/install.sh | DUX_INSTALL_DIR=~/.bin bash

# Specific version
curl -sSfL https://github.com/patrickdappollonio/dux/releases/latest/download/install.sh | DUX_VERSION=v0.1.0 bash
```

**Binary download:**

Grab the latest release for your platform from the [Releases](https://github.com/patrickdappollonio/dux/releases) page. Extract it, drop the `dux` binary somewhere on your `PATH`, and run it. On first launch, dux creates a fully commented config file. That file *is* the documentation.

## Prerequisites

- **`git`** — dux is built around git worktrees, so git is non-negotiable. If it's not on your PATH, dux won't get very far.
- **`gh` CLI** *(optional)* — authenticate it with your GitHub account and dux can pull PR statuses, check details, and show them right in the interface. Not required, but you'll miss it once you've tried it.

## How It Works

dux organizes work around **projects** (git repos) and **agents** (worktree sessions). When you create an agent, dux branches off a new git worktree so the agent has its own isolated copy of the code. No conflicts with your main checkout, no stepping on other agents' changes.

The interface has three panes:

- **Left:** your projects and agent sessions
- **Center:** the agent's live terminal output (or a file diff)
- **Right:** changed files, staging, and diffs

Tab between panes. Resize them with keyboard or mouse. Collapse the sidebar or git pane when you want more room. Go fullscreen with interactive mode. It's your layout.

### Bring Any CLI

Any terminal command can be a provider. The four defaults (Claude, Codex, Gemini, and OpenCode) are pre-configured, but adding your own is a config-only change:

```toml
[providers.my-agent]
command = "my-cool-agent"
args = ["--some-flag"]
resume_args = ["--continue"]
```

Set `resume_args` and dux can reconnect to detached or crashed sessions. Omit it if your CLI doesn't support resuming; dux will just relaunch it.

Cycle through providers on the fly with a single keypress, or set a default per-project.

### Macros

Tired of typing the same prompt over and over? Turn it into a macro. Macros are reusable text snippets you trigger from a quick-select bar. Search by name, hit enter, and the text gets pasted into the active pane.

```toml
[macros]
"Review" = { text = "review this code for bugs and security issues", surface = "agent" }
"Build" = { text = "cargo build --release 2>&1", surface = "terminal" }
"Ship it" = { text = "run all tests, fix failures, then commit", surface = "agent" }
```

Each macro can be scoped to the agent pane, the companion terminal, or both.

### Git Integration

The right pane is a full git staging area. Stage and unstage files, view syntax-highlighted diffs, write commit messages, push, and pull, all without leaving dux.

**AI commit messages:** Stage your changes, hit a key, and dux sends the diff to your provider in oneshot mode. It drafts a commit message using Conventional Commits, you tweak it (or don't), and commit. The prompt is fully customizable per-project.

**PR tracking:** With the `gh` CLI installed, dux tracks pull requests for your agent branches and shows status pills right in the interface.

### Companion Terminals

Each agent gets its own companion terminal: a separate shell session in the same worktree. Use it for builds, tests, git operations, or anything else you'd normally do in a terminal. You can spawn multiple companion terminals per agent.

### Forking Sessions

See an agent going down the wrong path? Fork it. dux creates a new worktree with the current files copied over so you can try a different approach without losing the original session. It's branching, but for your AI conversations.

### Command Palette

Press the palette key and you get fuzzy-searchable access to every action in dux, including features that don't have dedicated keybindings. Sort agents, toggle UI elements, open the resource monitor, rename sessions, edit macros, and more. If you forget a keybinding, just open the palette.

### Remote Share

Share a running dux session with a teammate (or your own laptop on the road) over an encrypted peer-to-peer link. No VPN, no SSH, no port forwarding — the connection is authenticated by a short pairing code and carried over QUIC via iroh's public relay mesh.

```bash
# On the machine running dux:
dux remote share

# A pairing code is printed; copy it and run on the other machine:
dux remote connect <code>
```

**Pass-leader control.** When a client connects, it takes the input lead: keystrokes typed on the client drive the host's dux session, while the host becomes view-only. The host's `Quit` keybinding is the one local escape hatch, so you can always exit. The host can reclaim the lead at any time with the `remote-take-lead` palette action, and can hand it back with `remote-release-lead`. When the client disconnects, the host regains control automatically. Set `client_leads_on_connect = false` in `[remote]` to keep the host in control on connect; set `allow_remote_input = false` to make every connection view-only regardless. If a client sends a lead-request while the host is driving, dux surfaces a status-bar notice — the host must explicitly release the lead to grant it; there is no automatic hand-off.

**Encryption and pairing.** Every byte on the wire is end-to-end encrypted by iroh's QUIC session, keyed by the host's ephemeral iroh identity. The pairing code bundles that identity with a 16-byte PIN the client must prove via HKDF-SHA256 before the host accepts input. The PIN itself never crosses the wire and is never written to `dux.log`. Pairing codes are single-use by construction — the host's endpoint accepts exactly one handshake attempt per code, then tears down — and `code_ttl_secs` (default 120s) bounds the accept/handshake wait, so a code that sits unused expires with `CodeExpired` rather than waiting indefinitely. Reusing a consumed code is refused because the old endpoint is gone.

**Headless hosts.** For a home server or unattended box, run `dux serve` — it prints the pairing code on stdout and has no TUI. Use `--code-file <path>` to write the code to a shared location instead of logs. Clients connect the same way with `dux remote connect <code>` from another machine.

**Browser clients.** Chromebooks, Android tablets with an external keyboard, and any other device that can't run a native `dux` binary connect through the browser client — a small static SPA that embeds an iroh endpoint as WebAssembly. The browser dials the host directly over iroh's relay mesh; there is no gateway, and no server of ours ever sees plaintext — the iroh QUIC session is end-to-end encrypted exactly as it is between two native dux peers.

Host the static bundle however you like:

- **Download** `dux-web-<version>.tar.gz` from the [Releases](https://github.com/patrickdappollonio/dux/releases) page and drop it on Netlify, Cloudflare Pages, S3, GitHub Pages, or any other static host.
- **Container (pre-built)** — `docker run -p 8080:80 ghcr.io/patrickdappollonio/dux-web:latest`. Published as a multi-arch image (`linux/amd64` + `linux/arm64`) so it runs natively on both Intel and Apple Silicon hosts via Docker Desktop.
- **Container (build from source)** — `docker build -t dux-web .` at the repo root. The top-level `Dockerfile` is a self-contained multi-stage build (Rust + wasm-pack for the bundle, nginx for the runtime) so you only need Docker installed locally; no Rust or clang on the host.
- **Build from source (native)** — `crates/dux-web-browser/build.sh` produces `dist/` locally. Faster than the Docker path but requires `wasm-pack` and `clang` on `$PATH`.

Open the hosted URL, paste a code from `dux remote share`, and type. The browser captures system shortcuts like Ctrl-W and Ctrl-T via the Keyboard Lock API (Chromium-based browsers) and forwards them to the host. Hold `Esc` for 500 ms to release the keyboard lock and disconnect.

**Config.** The `[remote]` section in `config.toml` exposes every knob:

```toml
[remote]
enabled = true                     # master switch for the subsystem
code_ttl_secs = 120                # pairing code validity window
allow_remote_input = true          # false = view-only sharing
client_leads_on_connect = true     # false = host keeps the lead on connect
# relay_url = "https://relay..."   # optional self-hosted relay override
```

The feature ships in the default build — there's no Cargo feature flag. Set `enabled = false` on machines that should never host a share.

### Configuration

The config file at `~/.config/dux/config.toml` (Linux) or `~/.dux/config.toml` (macOS) is exhaustively commented. Every setting is explained inline, so you should never need to leave the file to understand an option. Every keybinding is rebindable. Every pane width, scrollback limit, and default provider is configurable.

```bash
dux config path          # Print the config file path
dux config diff          # Show what you've changed from defaults
dux config diff --raw    # Unified diff against the default config
dux config reset         # Remove config and logs (keeps agents)
dux config reset --all   # Full factory reset
dux config regenerate    # Preview a fresh default config
```

Override the config directory with the `DUX_HOME` environment variable.

### Keybindings

All keybindings live in the `[keys]` section of the config. Key format supports single characters (`"j"`), special names (`"enter"`, `"pageup"`, `"shift-tab"`), and modifier combos (`"ctrl-d"`, `"ctrl-p"`). Each action takes an array of key combos:

```toml
[keys]
quit = ["ctrl-q"]
open_palette = ["ctrl-k"]
```

Press `?` in the app for the full keybinding reference. The help overlay is the authoritative source. This README intentionally doesn't list individual bindings because they're yours to change.

### Logging

Logs go to `dux.log` in the config directory. Control the level in your config:

```toml
[logging]
level = "info"   # "error", "info", or "debug"
path = "dux.log" # relative to config dir, or use an absolute path
```
