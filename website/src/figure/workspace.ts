// The fabricated workspace the homepage figure renders.
//
// Everything here is invented, and deliberately so: it is a plausible afternoon
// of work on a made-up storefront, not a capture of anyone's machine. The point
// is that the SHAPES are real. These are the exact `Spine` / `Bootstrap` /
// `ChangedFileView` types the server sends, so the real components read them
// exactly as they read live data, and if one of those contracts changes this
// file stops type-checking.
//
// No real company, product or person is named.

import type { Bootstrap } from "@/lib/bootstrapApi"
import type { Spine } from "@/lib/spineApi"
import type {
  AgentTabView,
  ChangedFileView,
  ProjectView,
  SessionView,
  TerminalView,
} from "@/lib/types"

// Fixed timestamps: a build-time render must be byte-identical from one build to
// the next, so nothing here may read the clock. These are only ever shown as
// relative ages by the components that use them.
const CREATED = "2025-03-11T09:12:00Z"
const UPDATED = "2025-03-11T14:41:00Z"

function project(over: Partial<ProjectView> & Pick<ProjectView, "id" | "name" | "path">): ProjectView {
  return {
    default_provider: "claude",
    explicit_default_provider: null,
    auto_reopen_agents: null,
    startup_command: null,
    env: {},
    current_branch: "main",
    branch_status: "clean",
    path_missing: false,
    leading_branch: "main",
    created_at: CREATED,
    terminals: [],
    ...over,
  }
}

function tab(id: string, provider: string, over: Partial<AgentTabView> = {}): AgentTabView {
  return {
    id,
    provider,
    order: 0,
    working: false,
    typing: false,
    needs_attention: false,
    has_output: true,
    has_live_process: true,
    ...over,
  }
}

function session(
  over: Partial<SessionView> &
    Pick<SessionView, "id" | "project_id" | "title" | "branch_name">,
): SessionView {
  const branch = over.branch_name
  return {
    provider: "claude",
    initial_branch: branch,
    source_branch: "main",
    worktree_path: `/home/dev/.local/share/dux/worktrees/${over.id}`,
    status: "active",
    auto_reopen_enabled: true,
    terminals: [],
    tabs: [tab(over.id, over.provider ?? "claude")],
    has_output: true,
    working: false,
    typing: false,
    needs_attention: false,
    created_at: CREATED,
    updated_at: UPDATED,
    last_focused_tab: null,
    ...over,
  }
}

function terminal(
  over: Partial<TerminalView> & Pick<TerminalView, "id" | "label">,
): TerminalView {
  return {
    has_output: true,
    working: false,
    typing: false,
    foreground_cmd: null,
    sort_order: 0,
    created_at: CREATED,
    updated_at: UPDATED,
    ...over,
  }
}

// --- The projects ----------------------------------------------------------

const STOREFRONT = "prj-storefront"
const BILLING = "prj-billing-api"
const DOCS = "prj-docs-site"

// A project terminal: owned by the project, spawned at its repo root, with no
// agent attached. The dev server has been left running in it.
const devServer = terminal({
  id: "term-dev-server",
  label: "Terminal 1",
  working: true,
  foreground_cmd: "npm run dev",
  sort_order: 1,
})

const projects: ProjectView[] = [
  project({
    id: STOREFRONT,
    name: "storefront",
    path: "/home/dev/code/storefront",
  }),
  project({
    id: BILLING,
    name: "billing-api",
    path: "/home/dev/code/billing-api",
    default_provider: "codex",
    current_branch: "main",
  }),
  project({
    id: DOCS,
    name: "docs-site",
    path: "/home/dev/code/docs-site",
    leading_branch: "trunk",
    current_branch: "trunk",
  }),
]

// --- The agents ------------------------------------------------------------

// The focused agent: mid-turn, two provider tabs against the one worktree, and
// an open pull request.
const CHECKOUT = "agt-checkout-retry"

const checkoutRetry = session({
  id: CHECKOUT,
  project_id: STOREFRONT,
  title: "checkout-retry",
  branch_name: "dux/checkout-retry",
  working: true,
  tabs: [
    tab(CHECKOUT, "claude", { order: 0, working: true }),
    tab("tab-checkout-review", "codex", { order: 1 }),
  ],
  pr: {
    number: 482,
    state: "open",
    title: "Retry a failed payment authorization once before surfacing it",
    url: "https://example.invalid/storefront/pull/482",
  },
})

const sessions: SessionView[] = [
  checkoutRetry,
  session({
    id: "agt-webhook-replay",
    project_id: BILLING,
    title: "webhook-replay",
    provider: "codex",
    branch_name: "dux/webhook-replay",
    needs_attention: true,
    tabs: [tab("agt-webhook-replay", "codex", { needs_attention: true })],
  }),
  session({
    id: "agt-invoice-pdf",
    project_id: BILLING,
    title: "invoice-pdf-export",
    provider: "codex",
    branch_name: "dux/invoice-pdf-export",
    // Idle: nothing running, nothing waiting on the user.
  }),
  session({
    id: "agt-search-ranking",
    project_id: STOREFRONT,
    title: "search-ranking",
    provider: "opencode",
    branch_name: "dux/search-ranking",
    status: "detached",
    typing: true,
    tabs: [
      tab("agt-search-ranking", "opencode", {
        typing: true,
        has_live_process: false,
      }),
    ],
  }),
]

// Terminals travel as ONE flat list, each carrying its owner, which is what the
// browser reads. They used to be nested inside the project or session that owns
// them, and this figure kept seeding them that way after the wire changed, so
// the sidebar's terminals section rendered empty and `verify-figure` caught it.
const terminals: TerminalView[] = [
  { ...devServer, owner: { kind: "project", project_id: STOREFRONT } },
  {
    ...terminal({
      id: "term-pytest-watch",
      label: "Terminal 2",
      foreground_cmd: null,
      sort_order: 2,
    }),
    owner: { kind: "session", session_id: "agt-invoice-pdf" },
  },
]

export const spine: Spine = {
  projects,
  sessions,
  terminals,
  sidebar: {
    groups: [
      {
        project_id: STOREFRONT,
        name: "storefront",
        orphaned: false,
        path_missing: false,
        session_ids: [CHECKOUT, "agt-search-ranking"],
      },
      {
        project_id: BILLING,
        name: "billing-api",
        orphaned: false,
        path_missing: false,
        session_ids: ["agt-webhook-replay", "agt-invoice-pdf"],
      },
      {
        project_id: DOCS,
        name: "docs-site",
        orphaned: false,
        path_missing: false,
        session_ids: [],
      },
    ],
    agentless_start: 2,
  },
}

export const bootstrap: Bootstrap = {
  available_providers: ["claude", "codex", "opencode", "copilot"],
  macros: [],
  welcome_tips: [],
  dux_version: "development",
  randomize_agent_names_by_default: false,
  gh_available: true,
  github_integration: true,
  copy_on_select: true,
  pr_banner_position: "top",
  agent_scrollback_lines: 10000,
  show_changes_pane: true,
  always_show_tab_strip: false,
  global_env: {},
  status_clear_seconds: 6,
  agent_sort: "active",
  title: "dux",
}

// The changed-files pane's contents for the focused agent: the diff a real
// retry-once change would leave behind, staged and unstaged.
export const stagedFiles: ChangedFileView[] = [
  {
    status: "M",
    path: "src/checkout/payment-intent.ts",
    additions: 64,
    deletions: 11,
    binary: false,
  },
  {
    status: "A",
    path: "src/checkout/retry-policy.ts",
    additions: 87,
    deletions: 0,
    binary: false,
  },
]

export const unstagedFiles: ChangedFileView[] = [
  {
    status: "M",
    path: "src/checkout/__tests__/payment-intent.test.ts",
    additions: 132,
    deletions: 4,
    binary: false,
  },
  {
    status: "M",
    path: "src/components/CheckoutSummary.tsx",
    additions: 9,
    deletions: 9,
    binary: false,
  },
  {
    status: "D",
    path: "src/checkout/legacy-authorize.ts",
    additions: 0,
    deletions: 218,
    binary: false,
  },
  {
    status: "R",
    path: "docs/payments/retries.md",
    additions: 21,
    deletions: 3,
    binary: false,
  },
]

/** The agent whose terminal and changed files the figure is showing. */
export const focusedSessionId = CHECKOUT
