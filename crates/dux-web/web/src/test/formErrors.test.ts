import { describe, expect, it } from "vitest"

import { takeFormErrors } from "./formErrors"

// The setup file fails every test that leaves one of these behind, so a dialog's
// own tests fail when a field it draws has no name or drops its wiring. A test
// that means to raise one silences `console.error` with a mock, which keeps it out.
describe("form development errors", () => {
  it("are kept for the test that raised them, and nothing else is", () => {
    console.error("FormGroup: the field has no name.")
    console.error("FormSection: the section has no title.")
    console.error("Warning: something else.")
    expect(takeFormErrors()).toEqual(["FormGroup: the field has no name.", "FormSection: the section has no title."])
    expect(takeFormErrors()).toEqual([])
  })
})
