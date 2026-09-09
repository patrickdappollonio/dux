// The site-wide title and meta description, imported by both Layout.astro's prop
// defaults and index.astro so every page describes the product one way.
//
// The description must say, in this order: what dux does, that it has two front
// ends over one workspace, and the no-protocol-layer point, which is the actual
// differentiator and never gets cut for length.

export const SITE_TITLE =
  "dux: run multiple Claude Code, Codex & Copilot agents in parallel, terminal or browser | getdux.app";

export const SITE_DESCRIPTION =
  "dux runs multiple AI coding agents (Claude Code, Codex, Copilot, OpenCode, or any CLI) in parallel, a git worktree each or straight in a folder you already have. Two front ends over one workspace: a terminal UI, and a web UI you start with dux server, both driving the same agents, phone included. Real CLIs, real PTYs, no protocol layers.";

// The blog and its RSS feed share a description too. Kept alongside the site
// strings so a change to how dux describes itself lands in one edit.
export const BLOG_DESCRIPTION =
  "Updates, release notes, and what's being worked on in dux, the workspace for running AI coding agents in parallel from a terminal or a browser.";
