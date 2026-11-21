// How an editor tab comes to act as the page that opened it.
//
// The standalone editor is a page load of its own with no terminal socket, so
// it drives nothing and the server would refuse its writes while the tab that
// opened it drives the agent. The opener therefore hands it the ids it acts as
// (`pageSeat.ts`), over `window.opener`: a message one window sends to exactly
// one other, both checked to be this origin. Nothing rides the address, the
// history or storage. A tab opened cold has no opener and is handed nothing.

import { useEffect, useSyncExternalStore } from "react"

import { fileApi } from "@/lib/fileApi"
import { rootKey, type EditorRoot } from "@/lib/editorRoot"
import { actingPageIds, actsForOpener, adoptOpenerPages } from "@/lib/pageSeat"

/// The `rel` of every link that opens the standalone editor in a new tab. A
/// `_blank` link drops the opener unless told to keep it, and the handshake
/// needs it; the address is always this app's own.
export const EDITOR_TAB_REL = "opener"

const ASK = "dux:pages?"
const TELL = "dux:pages"

/// How long an opener gets to answer before the tab goes on without it.
const ANSWER_WAIT_MS = 1500

function messageType(data: unknown): unknown {
  return typeof data === "object" && data !== null ? (data as { type?: unknown }).type : undefined
}

/// Answer a tab this page opened that asks which pages it may act as.
export function answerOpenedTabs(): () => void {
  const onMessage = (event: MessageEvent) => {
    if (event.origin !== window.location.origin || messageType(event.data) !== ASK) return
    const asking = event.source as Window | null
    if (asking === null) return
    let openedHere = false
    try {
      openedHere = asking.opener === window
    } catch {
      // A window that has left this origin: not ours to answer.
    }
    if (!openedHere) return
    asking.postMessage({ type: TELL, pages: actingPageIds() }, window.location.origin)
  }
  window.addEventListener("message", onMessage)
  return () => window.removeEventListener("message", onMessage)
}

function opener(): Window | null {
  const from = window.opener as Window | null | undefined
  return from != null && !from.closed ? from : null
}

/// Whether a page that opened this tab is still there to ask.
export function hasOpener(): boolean {
  return typeof window !== "undefined" && opener() !== null
}

/// Ask the opener which pages this tab may act as. Resolves true when the
/// answer differs from what the tab already held, false on the same answer or none.
export function askOpenerForPages(): Promise<boolean> {
  const from = hasOpener() ? opener() : null
  if (from === null) return Promise.resolve(false)
  return new Promise((resolve) => {
    const finish = (changed: boolean) => {
      window.removeEventListener("message", onMessage)
      window.clearTimeout(timer)
      resolve(changed)
    }
    const onMessage = (event: MessageEvent) => {
      if (event.origin !== window.location.origin || event.source !== from) return
      if (messageType(event.data) !== TELL) return
      const pages = (event.data as { pages?: unknown }).pages
      finish(Array.isArray(pages) && adoptOpenerPages(pages))
    }
    const timer = window.setTimeout(() => finish(false), ANSWER_WAIT_MS)
    window.addEventListener("message", onMessage)
    // Delivered only while the opener is still this origin's page.
    from.postMessage({ type: ASK }, window.location.origin)
  })
}

/// What the server says of this tab writing under a root while its row is
/// driven by `owner`: still asking, allowed as the opener's page, or refused.
export type OpenerVerdict = "asking" | "allowed" | "refused"

type Asked = { owner: string; verdict: OpenerVerdict }

// An immutable snapshot replaced on every change, as `useSyncExternalStore` needs.
let verdicts: Readonly<Record<string, Asked>> = {}
const listeners = new Set<() => void>()

function setVerdict(key: string, asked: Asked): void {
  verdicts = { ...verdicts, [key]: asked }
  for (const listener of listeners) listener()
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

const snapshot = () => verdicts

async function askServer(root: EditorRoot, owner: string): Promise<void> {
  const key = rootKey(root)
  setVerdict(key, { owner, verdict: "asking" })
  if (!actsForOpener()) await askOpenerForPages()
  let allowed = actsForOpener() && (await fileApi.writable(root))
  // An opener that reloaded is a new page: ask it again before believing a refusal.
  if (!allowed && (await askOpenerForPages())) allowed = await fileApi.writable(root)
  if (verdicts[key]?.owner !== owner) return
  setVerdict(key, { owner, verdict: allowed ? "allowed" : "refused" })
}

/// The server's word on whether this tab, acting as its opener, may write
/// under `root` while `owner` drives its row. Null when there is nothing to
/// ask: nobody else drives (`owner` null), or no page opened this tab.
export function useOpenerVerdict(root: EditorRoot, owner: string | null): OpenerVerdict | null {
  const key = rootKey(root)
  const asked = useSyncExternalStore(subscribe, snapshot, snapshot)[key]
  const ask = owner !== null && hasOpener()
  const current = asked !== undefined && asked.owner === owner
  useEffect(() => {
    if (ask && owner !== null && verdicts[key]?.owner !== owner) void askServer(root, owner)
  }, [ask, key, owner, root])
  if (!ask) return null
  return current ? asked.verdict : "asking"
}

/// Forget every answer, for tests.
export function resetOpenerVerdicts(): void {
  verdicts = {}
}
