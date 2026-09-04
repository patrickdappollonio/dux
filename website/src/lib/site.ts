// The site-wide title and meta description, in one place.
//
// Layout.astro's prop defaults and index.astro both import from here, so the
// homepage and every fallback page describe the product one way.
//
// What the description has to get right, in this order: what dux does, that it
// has TWO front ends over one workspace (a terminal UI and a web UI, both first
// class, neither one the other's remote control), and the no-protocol-layer
// point. The last one is the actual differentiator, so it never gets cut for
// length. Reaching the workspace from a phone is a consequence of the web front
// end, so it rides along at the end rather than leading.

export const SITE_TITLE =
  "dux: run multiple Claude Code, Codex & Copilot agents in parallel, terminal or browser | getdux.app";

export const SITE_DESCRIPTION =
  "dux runs multiple AI coding agents (Claude Code, Codex, Copilot, OpenCode, or any CLI) in parallel, a git worktree each or straight in a folder you already have. Two front ends over one workspace: a terminal UI, and a web UI you start with dux server, both driving the same agents, phone included. Real CLIs, real PTYs, no protocol layers.";

// The blog and its RSS feed share a description too. Kept alongside the site
// strings so a change to how dux describes itself lands in one edit.
export const BLOG_DESCRIPTION =
  "Updates, release notes, and what's being worked on in dux, the workspace for running AI coding agents in parallel from a terminal or a browser.";
