import { useState } from "react"
import { CopyIcon } from "lucide-react"

import { Button } from "@/components/ui/button"
import { copyToClipboard } from "@/lib/clipboard"
import { detailsOf, type Caught } from "@/lib/errorText"

// The "Copy details" control of an error screen, with the line that says how
// the copy went. A fragment: the caller's row holds the button and the line.
export function CopyDetails({ caught }: { caught: Caught }) {
  const [copy, setCopy] = useState<"idle" | "copied" | "failed">("idle")
  return (
    <>
      <Button
        variant="outline"
        className="h-10"
        onClick={() => {
          void copyToClipboard(detailsOf(caught)).then((ok) =>
            setCopy(ok ? "copied" : "failed"),
          )
        }}
      >
        <CopyIcon />
        Copy details
      </Button>
      <span role="status" className="text-sm text-muted-foreground">
        {copy === "copied"
          ? "Copied the error and where it happened."
          : copy === "failed"
            ? "Could not reach the clipboard; select the message above instead."
            : ""}
      </span>
    </>
  )
}
