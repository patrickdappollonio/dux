import { useState } from "react"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Textarea } from "@/components/ui/textarea"
import { closeCommit, socket, useDux } from "@/lib/store"

export function CommitDialog() {
  const { commitTarget } = useDux()
  const [message, setMessage] = useState("")

  const isOpen = commitTarget !== null

  function handleCommit() {
    if (!commitTarget || !message.trim()) return
    socket.sendCommand("commit_changes", { session_id: commitTarget, message: message.trim() })
    setMessage("")
    closeCommit()
  }

  function handleCancel() {
    setMessage("")
    closeCommit()
  }

  function handleOpenChange(open: boolean) {
    if (!open) handleCancel()
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false}>
        <DialogHeader>
          <DialogTitle>Commit changes</DialogTitle>
        </DialogHeader>
        <Textarea
          placeholder="Commit message…"
          value={message}
          onChange={(e) => setMessage(e.target.value)}
          className="min-h-24 resize-none"
          autoFocus
          onKeyDown={(e) => {
            if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
              e.preventDefault()
              handleCommit()
            }
          }}
        />
        <DialogFooter>
          <Button variant="outline" onClick={handleCancel}>
            Cancel
          </Button>
          <Button onClick={handleCommit} disabled={!message.trim()}>
            Commit
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
