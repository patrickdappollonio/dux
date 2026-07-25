## Quieter plumbing, louder failures

Version 0.6.0 is a tune-up release. No sprawling new subsystem this time — just dux getting more honest about environment variables, more careful about which branch it pulls from, and more forthcoming when a provider faceplants on launch. Plus the project finally grew a public face.

The theme: fewer surprises between you and a working agent. The agents bring quite enough surprises of their own.

### Environment config for agents and terminals

dux now has first-class env configuration.

Set global env once, override it per project when a repo needs special treatment, and dux passes those values into new agent PTYs, companion terminals, and startup commands.

```toml
[env]
EDITOR = "true"
API_KEY = "${FOOBAR_API_KEY}"

[[projects]]
path = "$HOME/projects/web-app"
name = "web-app"
env = { EDITOR = "nvim", NODE_ENV = "development" }
```

Env values support `$VAR` and `${VAR}` expansion, so you can pull secrets or machine-specific values from your parent environment without hardcoding them into config like a little security incident waiting to hatch. The config renderer, README, command palette, and storage all learned about these settings in the same change, so they stay documented, persisted, and applied consistently.

### Agents pull from the right branch now

Project refresh used to update whatever branch the source checkout happened to be parked on. Now it switches the source checkout back to the project's registered leading branch before pulling — and if that switch fails, it stops and hands you the actual git error instead of quietly updating the wrong branch.

The same branch contract runs through the neighboring flows too: pull-before-create uses the leading branch, the default-branch checkout action respects it, and branch status is judged against it. Fewer agents accidentally born from `feature/wip-oh-no` because your main checkout was in a mood.

### Provider launches fail louder, and earlier

When a provider command is missing or refuses to start, dux now fails fast and says so plainly. The check is generic, so your custom providers get the same honesty as the built-in ones.

A few related niceties tagged along:

* resuming appends resume arguments to the base provider args instead of replacing them, which behaves better with wrapper-style commands
* if a provider exits immediately with a short error, dux surfaces *that* message instead of just announcing the process exited
* the resume fallback path drops you straight into interactive fullscreen, so the agent is ready for input without an extra activation step

### The command palette speaks human now

The palette matched commands by their dashed names like `open-current-pr`. Type it the natural way — `open current pr` — and you used to get nothing, which rather defeats the point of a searchable palette.

Now both the query and the candidates are normalized (lowercased, runs of spaces and dashes collapsed to one), so `open current pr`, `open-current-pr`, and even a query littered with stray tabs and double spaces all land on the same command. Descriptions get the same treatment, and existing dashed queries keep working untouched.

### Agent exit output goes somewhere you can read it

When an agent dies fast with more output than the status line can hold, dux now writes the full captured output to the log before tearing down the PTY — and the status line stops pointing you at the agent pane for output that no longer lives there. The short failure message stays readable; the whole story waits in the log, exactly where the message says it is.

### A website exists now

[dux has a proper website now too](https://getdux.app/). Not the main headline for this release, but still nice: the project has somewhere cleaner to point people than “go read the README, yes the terminal app has lore.”


## What's Changed
* Bug Fix: Pull projects from their leading branch by @patrickdappollonio in https://github.com/patrickdappollonio/dux/pull/243
* Add environment variables for projects and agents by @patrickdappollonio in https://github.com/patrickdappollonio/dux/pull/244
* Improve provider launch failures by @patrickdappollonio in https://github.com/patrickdappollonio/dux/pull/245
* Match palette commands when query uses spaces by @patrickdappollonio in https://github.com/patrickdappollonio/dux/pull/246
* Log truncated agent exit output by @patrickdappollonio in https://github.com/patrickdappollonio/dux/pull/247
* Add the getdux.app documentation website by @patrickdappollonio in https://github.com/patrickdappollonio/dux/pull/248
* Add a documentation section to the website by @patrickdappollonio in https://github.com/patrickdappollonio/dux/pull/249
* Add repo subtitles and friendlier names to the Recommended tools cards by @patrickdappollonio in https://github.com/patrickdappollonio/dux/pull/250
* Keep the card badge from wedging between title and subtitle by @patrickdappollonio in https://github.com/patrickdappollonio/dux/pull/251



---

## Installation

Use any of the following options to install dux on your system:


**Homebrew (macOS and Linux):**

On macOS, Homebrew is the preferred route. This command taps the source and installs dux in one shot, because life's too short for a two-command install:

```bash
brew install patrickdappollonio/tap/dux
```

**npm:**

Install dux globally so the CLI lands on your `PATH`. Installing it as a dependency of some random project technically works, but that's not where terminal apps go to be useful:

```bash
npm install -g @patrickdappollonio/dux
dux
```

For a one-off run without keeping it around:

```bash
npx -y @patrickdappollonio/dux
```

**Shell (all platforms):**

The install script sniffs out your operating system and architecture, then grabs the matching release archive. No guessing which tarball has your name on it:

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

**Full Changelog**: https://github.com/patrickdappollonio/dux/compare/v0.5.0...v0.6.0
