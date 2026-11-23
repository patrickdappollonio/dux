import { Lock } from "lucide-react"

import { Notice } from "@/components/Notice"

// The editor's one statement that this page may read the files but not change
// them, because it does not drive the row the files belong to. It stays for as
// long as that holds and has no dismiss, since every write control under it is
// greyed for the same reason. The amber of the encoding and disk notices: a
// fact that decides what a save would do, said before anything is written.
export function EditorReadOnlyBanner({ reason }: { reason: string }) {
  return (
    <Notice shape="strip" tone="warning" icon={Lock}>
      {reason}
    </Notice>
  )
}
