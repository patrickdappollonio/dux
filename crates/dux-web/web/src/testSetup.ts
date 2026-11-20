// The one setup file every suite runs.
import { beforeEach } from "vitest"

import { forgetHeldConnIds } from "./lib/ptyOwnership"

// The ids a page has held are module state, so without this an id one test
// held reads as this page's own in the next.
beforeEach(() => forgetHeldConnIds())
