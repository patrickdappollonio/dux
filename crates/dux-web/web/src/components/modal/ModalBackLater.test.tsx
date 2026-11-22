// @vitest-environment jsdom
// The Back cases of Modal.test.tsx on a history whose traversals land on a
// later task with `navigation.transition` up meanwhile, as a browser's do.
import { vi } from "vitest"

import { installBrowserHistory } from "@/test/browserHistory"
import { describeModalBack } from "@/test/modalBackCases"

const browser = installBrowserHistory({ traversal: "later" })
vi.stubGlobal(
  "fetch",
  vi.fn(() => Promise.reject(new Error("offline test"))),
)
await import("@/lib/store")

describeModalBack(browser)
