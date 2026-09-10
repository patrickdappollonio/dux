// One idempotent seed for the whole screenshot scene: the project, the six
// agents and their tabs, the two terminals, the macros, the pull request, and
// the changed files each pane shows. Run it against a fresh container or an
// already-seeded one; it converges to the same workspace either way.
//
//   node seed.js
//
// reshoot.sh runs this before shooting anything. Every scene file is written
// against the workspace this leaves behind, so a scene that needs something
// else stages it for itself and puts it back.
const {
  agents,
  api,
  containerSh,
  get,
  project,
  setFixture,
  sleep,
  stage,
} = require("./lib.js")

const PROJECT_PATH = "/repos/demo-api"
const STANDALONE_FOLDER = "/root/design-notes"

// Creation order, which is what the sidebar's own ordering is pinned relative
// to, and the fixture each agent is born on.
const MANAGED_AGENTS = [
  ["review-billing", "attention"],
  ["refactor-cache", "working"],
  ["add-rate-limits", "working"],
  ["fix-login-redirect", "working"],
  ["polish-onboarding", "steady"],
]

// The three providers the tab-strip screenshots show on one agent.
const TAB_PROVIDERS = ["claude", "codex", "opencode"]

const MACROS = [
  { name: "Review", text: "Review the diff and call out anything risky.", surface: "agent" },
  { name: "Write tests", text: "Write tests for the change you just made.", surface: "agent" },
  { name: "Explain failure", text: "Explain the last failure in plain terms.", surface: "agent" },
  { name: "Lint", text: "Run the linter and fix what it reports.", surface: "agent" },
]

// The changed files the panes show. Written straight into each worktree, so the
// counts in the Changes pane are git's own answer rather than a fixture.
const README_ADDENDUM = `
## Preview notes

Seeded by the dux screenshot tool; this worktree is disposable.
Reshooting the docs rewrites it from scratch.
`

const RATE_LIMIT_MAIN = `import time

RATE_LIMIT = 60
WINDOW_SECONDS = 60


def allowed(bucket, now=None):
    now = now or time.time()
    bucket = [t for t in bucket if now - t < WINDOW_SECONDS]
    return len(bucket) < RATE_LIMIT, bucket

def main():
    print("hello from demo-api, now with rate limits")
`

function shellQuote(text) {
  return `'${text.replace(/'/g, `'\\''`)}'`
}

// A heredoc whose body is never expanded, so the file lands byte for byte.
function writeFile(path, body) {
  return `cat > ${shellQuote(path)} <<'DUXSEEDEOF'\n${body}DUXSEEDEOF\n`
}

async function seedProject() {
  const list = await get("/api/v1/projects")
  let demoApi = list.find((p) => p.path === PROJECT_PATH)
  if (!demoApi) {
    demoApi = await api("POST", "/api/v1/projects", { path: PROJECT_PATH, name: "demo-api" })
    console.log("added project demo-api")
  }
  // The fake provider is what makes working and idle states shootable without
  // authenticating a real CLI.
  await api("PATCH", `/api/v1/projects/${demoApi.id}`, { provider: "fake" })
  return demoApi
}

// The pull-request fixture needs a GitHub-looking remote to resolve against;
// the stand-in gh answers the lookup itself.
function seedRemote() {
  containerSh(
    `cd ${PROJECT_PATH}
     git remote remove origin 2>/dev/null || true
     git remote add origin https://github.com/acme/demo-api.git`,
  )
  console.log("origin remote pointed at acme/demo-api")
}

// The standalone agent's home is a plain folder with no repository in it, which
// is what makes its changes panel say so.
function seedStandaloneFolder() {
  containerSh(
    `mkdir -p ${STANDALONE_FOLDER}/src
     ${writeFile(`${STANDALONE_FOLDER}/NOTES.md`, "# Design notes\n")}`,
  )
}

async function seedManagedAgents(projectId) {
  const existing = await get("/api/v1/sessions")
  const have = new Set(existing.map((s) => s.title))
  for (const [name, fixture] of MANAGED_AGENTS) {
    if (have.has(name)) continue
    await setFixture(fixture)
    await api("POST", "/api/v1/sessions", {
      kind: "new",
      project_id: projectId,
      name,
      copy_uncommitted_changes: false,
    })
    console.log("created agent", name)
    await sleep(2500)
  }
}

async function seedStandaloneAgent() {
  const list = await get("/api/v1/sessions")
  const existing = list.find((s) => s.title === "design-notes")
  if (existing && existing.workspace.folder_path === STANDALONE_FOLDER) return
  if (existing) {
    // A folder agent from an older seed: dux removes its own record only, which
    // is exactly what is wanted here.
    await api("DELETE", `/api/v1/sessions/${existing.id}`)
    await sleep(1000)
  }
  await api("POST", "/api/v1/sessions", {
    kind: "standalone",
    folder: STANDALONE_FOLDER,
    name: "design-notes",
    provider: "claude",
  })
  console.log("created standalone agent design-notes")
  await sleep(3000)
}

// refactor-cache carries one tab per provider, which is what the tab strip and
// the tab menu are shot against.
async function seedTabs() {
  const by = await agents()
  const session = by["refactor-cache"]
  for (let i = session.tabs.length; i < TAB_PROVIDERS.length; i++) {
    await api("POST", `/api/v1/sessions/${session.id}/tabs`, { provider: TAB_PROVIDERS[i] })
    await sleep(1800)
  }
  const now = (await get("/api/v1/sessions")).find((s) => s.id === session.id)
  for (let i = 0; i < now.tabs.length && i < TAB_PROVIDERS.length; i++) {
    if (now.tabs[i].provider === TAB_PROVIDERS[i]) continue
    await api("PATCH", `/api/v1/sessions/${session.id}/tabs/${now.tabs[i].id}`, {
      provider: TAB_PROVIDERS[i],
    })
  }
  console.log("refactor-cache tabs:", TAB_PROVIDERS.join(", "))
}

// One terminal the project owns and one standalone in $HOME, in that order: the
// editor scene addresses the project's by the id the first spawn hands out.
async function seedTerminals(projectId) {
  const spine = await get("/api/v1/workspace")
  const terminals = spine.terminals || []
  if (!terminals.some((t) => t.owner.kind === "project")) {
    await api("POST", `/api/v1/projects/${projectId}/terminals`)
    await sleep(800)
  }
  if (!terminals.some((t) => t.owner.kind === "standalone")) {
    await api("POST", "/api/v1/terminals")
    await sleep(800)
  }
  const after = await get("/api/v1/workspace")
  console.log("terminals:", (after.terminals || []).map((t) => t.id).join(", "))
}

async function seedPullRequest() {
  const by = await agents()
  const session = by["fix-login-redirect"]
  if (session.pr && session.pr.number) return
  await api("PUT", `/api/v1/sessions/${session.id}/pull-request`, { pr: "acme/demo-api#123" })
  await sleep(4000)
  const now = (await get("/api/v1/sessions")).find((s) => s.id === session.id)
  console.log("pull request:", JSON.stringify(now.pr))
}

// Changed files, written into two worktrees: the login fix's four-file set that
// the Changes pane and its bulk bar are shot against, and the rate-limit rewrite
// the editor's diff view shows.
function seedChangedFiles() {
  const worktree = (branch) => `/data/dux/worktrees/demo-api/${branch}`
  containerSh(
    `cd ${worktree("fix-login-redirect")}
     ${writeFile("NOTES.md", "The redirect handler re-resolves the session on every hop.\n")}
     ${writeFile("src/cache.ts", "export const sessionTtlSeconds = 900\n")}
     ${writeFile("src/redirect.ts", "export const maxRedirectHops = 3\n")}
     printf '%s' ${shellQuote(README_ADDENDUM)} >> README.md`,
  )
  containerSh(
    `cd ${worktree("add-rate-limits")}
     ${writeFile("src/main.py", RATE_LIMIT_MAIN)}
     ${writeFile("src/retry.rs", "pub fn retry_limit() -> usize {\n    3\n}\n")}
     printf '%s' ${shellQuote(README_ADDENDUM)} >> README.md`,
  )
  console.log("changed files written into both worktrees")
}

async function main() {
  const demoApi = await seedProject()
  seedRemote()
  seedStandaloneFolder()
  await seedManagedAgents(demoApi.id)
  await seedStandaloneAgent()
  await seedTabs()
  await seedTerminals(demoApi.id)
  await api("PUT", "/api/v1/macros", { entries: MACROS })
  await seedPullRequest()
  seedChangedFiles()
  console.log("staging the sidebar")
  await stage("all-working")
  console.log("seed complete")
}

main().catch((error) => {
  console.error(String(error))
  process.exit(1)
})
