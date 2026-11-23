import { Lock } from "lucide-react"

// The editor's one statement that this page may read the files but not change
// them, because it does not drive the row the files belong to. It stays for as
// long as that holds and has no dismiss, since every write control under it is
// greyed for the same reason. The amber of the encoding and disk notices: a
// fact that decides what a save would do, said before anything is written.
export function EditorReadOnlyBanner({ reason }: { reason: string }) {
  return (
    <div
      role="status"
      data-toast-clear
      className="flex shrink-0 items-center gap-2 border-b border-dux-warning/40 bg-dux-warning/10 px-3 py-2 text-sm"
    >
      <Lock className="size-4 shrink-0 text-dux-warning" />
      <span className="min-w-0 flex-1">{reason}</span>
    </div>
  )
}
