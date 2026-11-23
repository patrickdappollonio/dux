// @vitest-environment jsdom
// The same cases on a history whose traversals land on a later task.
import { vi } from "vitest"

import { installBrowserHistory } from "@/test/browserHistory"
import { describeSpentBack } from "@/test/spentBackCases"

const browser = installBrowserHistory({ traversal: "later" })
history.pushState(null, "", "/#/page")
vi.stubGlobal("fetch", vi.fn(() => Promise.reject(new Error("offline test"))))
vi.spyOn(console, "warn").mockImplementation(() => {})
await import("@/lib/store")

describeSpentBack(browser)
