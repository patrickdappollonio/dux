import { TriangleAlert } from "lucide-react"
import type { ReactNode, Ref } from "react"

import { Notice } from "@/components/Notice"

// A save the server refused because what the dialog read has changed since,
// and nothing was saved. A choice, not an error: reloading throws the user's
// edits away, so "Keep editing" has focus and the reload is the destructive one.
export function StaleSaveNotice({
  message,
  reloadLabel,
  onKeepEditing,
  onReload,
  keepEditingRef,
}: {
  message: ReactNode
  reloadLabel: string
  onKeepEditing: () => void
  onReload: () => void
  keepEditingRef?: Ref<HTMLButtonElement>
}) {
  return (
    <Notice
      // The Macros dialog finds the box by this when it decides where focus
      // may stay.
      data-stale-save=""
      shape="box"
      tone="danger"
      icon={TriangleAlert}
      actions={[
        {
          label: "Keep editing",
          onSelect: onKeepEditing,
          autoFocus: true,
          buttonRef: keepEditingRef,
        },
        { label: reloadLabel, onSelect: onReload, look: "danger" },
      ]}
    >
      {message}
    </Notice>
  )
}
