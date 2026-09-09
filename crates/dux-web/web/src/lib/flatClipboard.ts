import { notifyError, notifySuccess } from "./notify"

import { copyToClipboard } from "@/lib/clipboard"

export function clipboardWorktree(path: string): void {
  void copyToClipboard(path).then((ok) =>
    ok
      ? notifySuccess("Copied local path to clipboard")
      : notifyError("Couldn't copy the path"),
  )
}
