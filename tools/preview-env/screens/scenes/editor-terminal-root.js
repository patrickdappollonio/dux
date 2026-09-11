// The editor rooted at a terminal's own directory rather than at an agent's
// worktree: the header names the terminal, and the tree is the repo root it was
// spawned in.
const {
  clearToasts,
  expectVisible,
  expectVisibleText,
  get,
  goto,
  project,
  sleep,
} = require("../lib.js")

module.exports = {
  file: "editor-terminal-root.png",
  viewport: "desktop",
  async shoot(page) {
    const demoApi = await project("/repos/demo-api")
    const spine = await get("/api/v1/workspace")
    const terminal = (spine.terminals || []).find(
      (t) => t.owner.kind === "project" && t.owner.project_id === demoApi.id,
    )
    if (!terminal) throw new Error("the seed left no project terminal to root the editor at")
    await goto(
      page,
      `#/editor/project/${encodeURIComponent(demoApi.id)}/terminal/${encodeURIComponent(terminal.id)}/file/README.md`,
    )
    await sleep(4000)
    await clearToasts(page)
    // The tree rooted at the repository, the file the address opened, and an
    // editor that mounted rather than a frame still waiting for monaco.
    await expectVisibleText(page, "README.md", { what: "the opened file" })
    await expectVisibleText(page, "src", { what: "the repository's own tree" })
    await expectVisible(page, ".monaco-editor", "the editor")
    return { x: 0, y: 0, width: 1440, height: 900 }
  },
}
