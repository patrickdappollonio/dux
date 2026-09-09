// The one place the app reloads itself, in its own module so the decision that leads
// here is testable without jsdom trying to navigate. Reserved for the reconnect path
// finding a different run or build, where carrying on means rendering a new server's
// data through old code; nothing else may reload the window.
//
// The editor's beforeunload guard in `editorDrafts.ts` is disarmed first, because
// this reload is silent: no prompt, no toast, no banner. Drafts live in page memory
// and are lost across it, deliberately.
import { disarmBeforeUnloadGuard } from "./editorDrafts"

export function reloadPage(): void {
  disarmBeforeUnloadGuard()
  window.location.reload()
}
