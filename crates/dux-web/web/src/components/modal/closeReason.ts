import type { ModalCloseReason } from "./types"

/** The way out a base-ui close request was, or null for one no person took
 * (an imperative close, or none named). A focus leaving the popup is a press
 * elsewhere as far as the caller is concerned. */
export function closeReasonFrom(reason: string): ModalCloseReason | null {
  switch (reason) {
    case "escape-key":
      return "escape"
    case "outside-press":
    case "focus-out":
      return "outside"
    case "close-press":
      return "close-control"
    default:
      return null
  }
}
