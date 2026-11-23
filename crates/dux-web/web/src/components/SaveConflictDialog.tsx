import { Modal } from "@/components/modal"
import { InlineCode } from "@/components/ui/inline-code"
import type { SaveEncoding } from "@/lib/editorEncoding"

export interface SaveConflictTarget {
  tabId: string
  path: string
  // The text the refused save was carrying, kept so "Overwrite" can re-send
  // exactly what the user asked to save rather than re-reading the draft,
  // which may have moved on.
  body: string
  // The encoding the refused save was writing in, so "Overwrite" writes the
  // same bytes the person asked for.
  encoding: SaveEncoding
  // The file is gone rather than merely different: there is nothing to reload,
  // so the offer is to write it back or to cancel.
  deleted: boolean
}

interface SaveConflictDialogProps {
  target: SaveConflictTarget | null
  onOverwrite: () => void
  onReload: () => void
  onClose: () => void
  // Another operation is out on the file (lib/editorLane.ts): both offers
  // would start a second one, so they wait for it.
  busy?: boolean
  // Why this page may not write here now (another device drives the row), or
  // null. Overwriting writes to disk; reloading only discards the buffer.
  writeBlocked?: string | null
}

// What a refused save looks like to the user: the file is not what this buffer
// was read from, so somebody else's work is on disk. Three honest answers, all
// offered: keep this version and overwrite theirs, take the disk version through
// its own destructive confirm, or do nothing. Cancel takes focus, doing nothing
// being the only choice that loses no work.
//
// Deliberately not a toast: a toast retires itself, and every option here
// destroys something.
export function SaveConflictDialog({
  target,
  onOverwrite,
  onReload,
  onClose,
  busy = false,
  writeBlocked = null,
}: SaveConflictDialogProps) {
  const path = target?.path ?? ""
  const deleted = target?.deleted ?? false

  // Both offers would start a second operation on the file, so they wait.
  const busyReason = busy
    ? "Another action on this file is still running. Wait for it to finish."
    : null
  return (
    <Modal
      kind="confirm"
      open={target !== null}
      onClose={onClose}
      // Raised by the server refusing a save, not by a click, so it takes no
      // history entry for the browser's Back.
      historyEntry={false}
      title={deleted ? "That file is gone" : "The file changed on disk"}
      cancelLabel="Cancel"
      alternative={
        deleted
          ? undefined
          : {
              label: "Reload from disk",
              tone: "plain",
              disabledReason: busyReason,
              onSelect: onReload,
            }
      }
      confirm={{
        label: deleted ? "Write it back" : "Overwrite",
        tone: "destructive",
        disabledReason: writeBlocked ?? busyReason,
        onSelect: onOverwrite,
      }}
    >
      <p className="text-sm text-destructive">
        {deleted ? (
          <>
            <InlineCode>{path}</InlineCode> was deleted
            after you opened it, so nothing was saved. You can write your
            version back as a new file at the same path, or cancel and keep
            the text here.
          </>
        ) : (
          <>
            <InlineCode>{path}</InlineCode> changed after
            you opened it, so nothing was saved. Saving anyway replaces
            whatever is on disk now; reloading replaces what you typed.
          </>
        )}
      </p>
    </Modal>
  )
}
