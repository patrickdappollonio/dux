// @vitest-environment jsdom
import { vi } from "vitest"

import { installBrowserHistory } from "@/test/browserHistory"
import { describeSpentBack } from "@/test/spentBackCases"

const browser = installBrowserHistory()
history.pushState(null, "", "/#/page")
vi.stubGlobal("fetch", vi.fn(() => Promise.reject(new Error("offline test"))))
vi.spyOn(console, "warn").mockImplementation(() => {})
await import("@/lib/store")

describeSpentBack(browser)
