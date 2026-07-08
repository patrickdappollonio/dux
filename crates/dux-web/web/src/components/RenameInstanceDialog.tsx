import { useState } from "react"
import { Check } from "lucide-react"
import { toast } from "sonner"

import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import {
  DEFAULT_FAVICON_HREF,
  FAVICON_COLORS,
  duckFaviconDataUri,
} from "@/lib/favicon"
import { resolveInstanceTitle } from "@/lib/instanceTitle"
import {
  closeRenameInstance,
  setInstanceIdentity,
  useDux,
} from "@/lib/store"
import { cn } from "@/lib/utils"

// The swatch grid. "Original" (favicon "") previews the bundled full-colour duck;
// each curated colour previews the tinted duck silhouette. The value written to
// config is the raw favicon string: "" for Original, or the colour name.
const SWATCHES: { value: string; label: string; href: string }[] = [
  { value: "", label: "Original", href: DEFAULT_FAVICON_HREF },
  ...Object.entries(FAVICON_COLORS).map(([name, hex]) => ({
    value: name,
    label: name.charAt(0).toUpperCase() + name.slice(1),
    href: duckFaviconDataUri(hex),
  })),
]

// Seed the selected favicon from the configured value: a curated name is kept as
// the selected swatch, anything else (unset, or a legacy hex/URL) falls back to
// "Original".
function seedFavicon(raw: string | null | undefined): string {
  const value = (raw ?? "").trim().toLowerCase()
  return FAVICON_COLORS[value] ? value : ""
}

function RenameInstanceForm() {
  const { bootstrap } = useDux()
  const [title, setTitle] = useState(() =>
    resolveInstanceTitle(bootstrap?.title),
  )
  const [favicon, setFavicon] = useState(() => seedFavicon(bootstrap?.favicon))

  // Refuse to write before the config is loaded: the form seeds its fields from
  // `bootstrap`, so saving with a null bootstrap would persist the fallback
  // defaults over whatever the operator actually configured (mirrors the macros
  // dialog's `saveMacros` guard). In practice the palette command that opens this
  // dialog only exists once bootstrap has loaded, so this is defense-in-depth.
  // Concurrent edits from another tab are last-writer-wins by design (a cosmetic
  // instance name in a single-tenant workspace does not warrant conflict UI).
  const save = () => {
    if (!bootstrap) {
      toast.error("Instance settings aren't loaded yet — try again in a moment.")
      return
    }
    setInstanceIdentity({ title, favicon })
    closeRenameInstance()
  }

  const reset = () => {
    if (!bootstrap) {
      toast.error("Instance settings aren't loaded yet — try again in a moment.")
      return
    }
    setInstanceIdentity({ title: "", favicon: "" })
    closeRenameInstance()
  }

  return (
    <DialogContent>
      <DialogHeader>
        <DialogTitle>Rename this instance</DialogTitle>
        <DialogDescription>
          Set the browser tab title and favicon colour for this dux instance.
          Saved to config and applied to every connected browser.
        </DialogDescription>
      </DialogHeader>

      <div className="flex flex-col gap-2">
        <label htmlFor="instance-title" className="text-sm font-medium">
          Name
        </label>
        <Input
          id="instance-title"
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          placeholder="dux"
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault()
              save()
            }
          }}
        />
      </div>

      <div className="flex flex-col gap-2">
        <span className="text-sm font-medium">Favicon</span>
        <div className="grid grid-cols-6 gap-2">
          {SWATCHES.map((swatch) => {
            const selected = swatch.value === favicon
            return (
              <SimpleTooltip key={swatch.value || "original"} content={swatch.label}>
                <button
                  type="button"
                  aria-label={swatch.label}
                  aria-pressed={selected}
                  onClick={() => setFavicon(swatch.value)}
                  className={cn(
                    "relative flex aspect-square items-center justify-center rounded-lg border bg-muted/40 p-1.5 transition-colors max-md:min-h-10 hover:bg-muted",
                    selected
                      ? "border-ring ring-3 ring-ring/50"
                      : "border-input",
                  )}
                >
                  <img
                    src={swatch.href}
                    alt=""
                    className="size-full object-contain"
                  />
                  {selected && (
                    <span className="absolute -top-1.5 -right-1.5 flex size-4 items-center justify-center rounded-full bg-primary text-primary-foreground">
                      <Check className="size-3" />
                    </span>
                  )}
                </button>
              </SimpleTooltip>
            )
          })}
        </div>
      </div>

      {/* Misclick-safe spacing between the swatches and the footer buttons. */}
      <div className="h-2" />
      <DialogFooter className="sm:justify-between">
        <Button variant="ghost" onClick={reset}>
          Reset to default
        </Button>
        <div className="flex flex-col-reverse gap-2 sm:flex-row">
          <Button variant="outline" autoFocus onClick={closeRenameInstance}>
            Cancel
          </Button>
          <Button onClick={save}>Save</Button>
        </div>
      </DialogFooter>
    </DialogContent>
  )
}

export function RenameInstanceDialog() {
  const { renameInstanceOpen } = useDux()

  return (
    <Dialog
      open={renameInstanceOpen}
      onOpenChange={(o) => {
        if (!o) closeRenameInstance()
      }}
    >
      {renameInstanceOpen && <RenameInstanceForm />}
    </Dialog>
  )
}
