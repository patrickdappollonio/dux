---
title: Recommended tools
description: Providers, MCP servers, skills, and companion CLIs that pair well with a dux workflow.
group: Reference
order: 90
---

dux runs your real CLI in a real terminal, with no protocol layer in between. So
whatever providers, MCP servers, skills, and command-line tools you already use
keep working inside a dux agent exactly as they do in your shell. This page
collects the ones that pair especially well with a multi-agent workflow.

It's community-curated. If something here is missing, use the **Edit this page on
GitHub** button at the bottom to open a pull request.

## AI CLIs (providers)

These are the CLIs that power agents. dux ships with several configured out of the
box, and any other CLI that supports an interactive session plus a headless
one-shot mode can be added (see [Custom CLI Agents](/docs/custom-agents)).

- **Claude Code**, Anthropic's coding CLI.
- **Codex**, OpenAI's coding CLI.
- **OpenCode**, an open-source coding agent.
- **Gemini CLI**, Google's coding CLI.

A GitHub Copilot CLI entry also ships preconfigured. To wire up your own, see
[Creating agents](/docs/creating-agents) and
[Custom CLI Agents](/docs/custom-agents).

## MCP servers

Model Context Protocol servers extend what an agent can reach: databases,
browsers, issue trackers, internal APIs, and more. Because dux doesn't sit between
you and the CLI, you configure MCP servers in your CLI the same way you always
have, and they work unchanged inside a dux agent.

Got an MCP server you rely on in your dux sessions? Add it here with a name, a
one-line description, and a link.

## Skills

Skills are reusable instructions and workflows your CLI loads (for example, Claude
Code's Agent Skills). Since dux launches the real CLI, your skills apply exactly as
they do outside dux, per agent and per worktree.

Have a skill that shines in a parallel-agent setup? Add it to this list.

## Companion CLIs

Small command-line tools that make agent sessions smoother.

- **gh** (the GitHub CLI). dux uses it for pull-request tracking, creating agents
  from PRs, and agent-opened PRs. Install it from
  [cli.github.com](https://cli.github.com); without it, dux quietly disables
  anything GitHub.

## Adding a recommendation

This page lives as Markdown in the dux repository. Use the **Edit this page on
GitHub** button below to open a pull request. Keep each entry short: a name, a
one-line description of what it does, and a link.
