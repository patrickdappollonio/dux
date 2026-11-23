import { createContext } from "react"

import type { ModalRow } from "./types"

/** How the app answers why this device may not change a row right now (null
 * when it may), handed to every modal from the app's root. A hook, read from
 * context so the modal itself never loads the app's store: a modal rendered
 * with no answer given (a test, a page outside the app) gates nothing. */
export const RowReasonContext = createContext<((row: ModalRow) => string | null) | null>(null)
