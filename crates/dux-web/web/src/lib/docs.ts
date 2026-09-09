// External documentation links, so every dialog that links out shares one base URL
// and set of section anchors. The anchors are github-slugger slugs of headings in
// website/docs/agent-tabs.md, which live in a separate package: renaming one there
// breaks these silently, and docs.test.ts is what catches it.
const DOCS_BASE = "https://getdux.app/docs"
const DOCS_AGENT_TABS = `${DOCS_BASE}/agent-tabs`

// "### Closing a tab is one-way" answers "why can't this be reopened?".
export const DOCS_AGENT_TABS_CLOSING = `${DOCS_AGENT_TABS}#closing-a-tab-is-one-way`
// "## How resume works": resume vs fresh, and why it's per-provider.
export const DOCS_AGENT_TABS_RESUME = `${DOCS_AGENT_TABS}#how-resume-works`
