import { AttachedSection } from "@/components/AttachedSection"
import { guardedConfirmAction } from "@/components/guardedConfirmAction"
import { Modal } from "@/components/modal"
import { useAttachedOverride } from "@/hooks/use-attached-override"
import { InlineCode } from "@/components/ui/inline-code"
import {
  closeDeleteTerminal,
  deleteTerminal,
  useDux,
} from "@/lib/store"
import { ownerRefFromWire, ownerSessionId } from "@/lib/terminalOwner"
import { terminalForeground } from "@/lib/terminals"
import type { TerminalView } from "@/lib/types"

// Confirmation before closing a companion terminal, whose running process is
// killed. Cancel takes focus, as in the TUI, and the row's ✕ opens this rather
// than deleting on a single click.
export function ConfirmDeleteTerminalDialog() {
  const { deleteTerminalTarget, spine } = useDux()

  // Derived from the ViewModel so a process that exits while the dialog is open
  // closes it through the effect below. One flat collection, so this is a lookup
  // by id and cannot miss an owner kind.
  const terminal: TerminalView | undefined =
    deleteTerminalTarget && spine
      ? spine.terminals.find((t) => t.id === deleteTerminalTarget)
      : undefined

  // Closes the dialog when the terminal vanishes from the ViewModel (its
  // process exited).
  const isOpen = deleteTerminalTarget !== null && terminal !== undefined
  const { blockers, pending, cancelRef, confirm } = useAttachedOverride(
    isOpen,
    deleteTerminalTarget,
  )
  // The title names the STATIC label like the TUI's prompt does ("delete
  // Terminal 1?"); the running command appears in the warning body instead,
  // avoiding the redundant "Close vim?" + "vim is running…" phrasing.
  const title = terminal?.label ?? ""
  // Closing changes the row the terminal belongs to: its agent's, or its own.
  const sessionId = terminal ? ownerSessionId(ownerRefFromWire(terminal.owner)) : null
  const row =
    terminal === undefined ? null : sessionId !== null ? { sessionId } : { terminalId: terminal.id }
  const foreground = terminal ? terminalForeground(terminal) : null

  async function handleConfirm() {
    if (!deleteTerminalTarget) return
    const target = deleteTerminalTarget
    const done = await confirm((accepted) => deleteTerminal(target, accepted))
    if (done) closeDeleteTerminal()
  }

  // Only an app running in the foreground is worth warning about: an idle
  // shell confirms with just the title.
  const body = (
    <>
      {foreground ? (
        <p className="text-sm text-destructive">
          <InlineCode>{foreground}</InlineCode> is running in this
          terminal and will be killed.
        </p>
      ) : null}
      {blockers?.length ? <AttachedSection blockers={blockers} /> : null}
    </>
  )
  return (
    <Modal
      kind="confirm"
      open={deleteTerminalTarget !== null}
      vanished={terminal === undefined}
      onClose={closeDeleteTerminal}
      row={row}
      title={
        <>
          Close <InlineCode>{title}</InlineCode>?
        </>
      }
      cancelLabel="Cancel"
      safeRef={cancelRef}
      confirm={guardedConfirmAction({
        verb: "Close terminal",
        blockers,
        pending,
        onConfirm: () => void handleConfirm(),
      })}
    >
      {foreground || blockers?.length ? body : null}
    </Modal>
  )
}
