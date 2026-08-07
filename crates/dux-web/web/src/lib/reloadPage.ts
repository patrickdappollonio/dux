// The one place the app reloads itself, kept in its own module so the decision
// that leads here can be tested without jsdom trying to navigate.
//
// There is exactly one caller: the reconnect path in `store.ts`, when the server
// it got back to turns out to be a different run or a different build from the
// one the tab loaded against (see `buildApi.ts`). Nothing else should reload the
// window: a reload throws away whatever the user had open, so it is reserved for
// the case where carrying on means rendering a new server's data through old
// code.
//
// The editor's beforeunload guard (armed while an editor tab is dirty, see
// `editorDrafts.ts`) is disarmed FIRST: this reload is silent by tenet — no
// prompt, no toast, no banner — so it must win over the guard. Drafts are
// lost across it, deliberately; they live in page memory, and a stale page
// must never keep running just to keep them.
import { disarmBeforeUnloadGuard } from "./editorDrafts"

export function reloadPage(): void {
  disarmBeforeUnloadGuard()
  window.location.reload()
}
