// External documentation links (getdux.app), so the two dialogs that link out
// share one source of truth for the base URL and section anchors.
//
// The anchors are rehype-slug (github-slugger) slugs of headings in
// website/docs/agent-tabs.md. They live in a separate package, so renaming a
// heading there would silently break these; docs.test.ts guards the exact
// headings these point at.
const DOCS_BASE = "https://getdux.app/docs"
const DOCS_AGENT_TABS = `${DOCS_BASE}/agent-tabs`

// "### Closing a tab is one-way" — answers "why can't this be reopened?".
export const DOCS_AGENT_TABS_CLOSING = `${DOCS_AGENT_TABS}#closing-a-tab-is-one-way`
// "## How resume works" — resume vs fresh, and why it's per-provider.
export const DOCS_AGENT_TABS_RESUME = `${DOCS_AGENT_TABS}#how-resume-works`
