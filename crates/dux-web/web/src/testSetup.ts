// The one setup file every suite runs.
import { afterEach, beforeEach } from "vitest"

import { forgetHeldConnIds } from "./lib/ptyOwnership"
import { takeFormErrors, watchFormErrors } from "./test/formErrors"

// The ids a page has held are module state, so without this an id one test
// held reads as this page's own in the next.
beforeEach(() => forgetHeldConnIds())

// A field drawn with no name, or with its wiring dropped, fails the test that drew it.
watchFormErrors()
afterEach(() => {
  const found = takeFormErrors()
  if (found.length > 0) throw new Error(`A form field reported a development error:\n${found.join("\n")}`)
})
