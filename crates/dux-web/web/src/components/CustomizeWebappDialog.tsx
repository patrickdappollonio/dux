import { useRef, useState } from "react"
import { Check } from "lucide-react"
import { toast } from "sonner"

import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
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
  changesPaneVisible,
  closeCustomizeWebapp,
  setChangesPaneVisibility,
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

function CustomizeWebappForm({
  saving,
  setSaving,
  savingRef,
}: {
  saving: boolean
  setSaving: (v: boolean) => void
  savingRef: React.RefObject<boolean>
}) {
  const dux = useDux()
  const { bootstrap } = dux
  const [title, setTitle] = useState(() =>
    resolveInstanceTitle(bootstrap?.title),
  )
  const [favicon, setFavicon] = useState(() => seedFavicon(bootstrap?.favicon))
  // The checkbox renders the LIVE effective visibility until the user touches
  // it (`paneChoice` stays null), so a toggle made by another connected client
  // while this dialog is open is reflected in the UI rather than silently
  // reverted on Save. Once touched, the user's explicit choice wins, and that
  // is the only case Save persists.
  const [paneChoice, setPaneChoice] = useState<boolean | null>(null)
  const livePaneVisible = changesPaneVisible(dux)
  const showChanges = paneChoice ?? livePaneVisible

  // The in-flight guard (savingRef/saving) is owned by the outer dialog so it
  // also gates onOpenChange; see the comment there.

  // Refuse to write before the config is loaded: the form seeds its fields from
  // `bootstrap`, so saving with a null bootstrap would persist the fallback
  // defaults over whatever the operator actually configured (mirrors the macros
  // dialog's `saveMacros` guard). In practice the palette command that opens this
  // dialog only exists once bootstrap has loaded, so this is defense-in-depth.
  // Concurrent edits from another tab are last-writer-wins by design (a cosmetic
  // instance name in a single-tenant workspace does not warrant conflict UI).
  // Save/Reset fire one or two independent persists (identity, and the Changes
  // pane only when the user touched the checkbox to a value differing from the
  // live one). The dialog closes only when every fired write succeeded; on a
  // partial failure it stays open (the store's own error toast says why) so
  // the user can retry instead of losing half the edit behind a closed dialog.
  const save = async () => {
    if (savingRef.current) return
    if (!bootstrap) {
      toast.error("Instance settings aren't loaded yet — try again in a moment.")
      return
    }
    savingRef.current = true
    setSaving(true)
    try {
      const writes = [setInstanceIdentity({ title, favicon })]
      // The Changes pane preference applies on Save, not live, matching the
      // rest of this form — and only when the user actually made a choice.
      if (paneChoice !== null && paneChoice !== livePaneVisible) {
        writes.push(setChangesPaneVisibility(paneChoice))
      }
      if ((await Promise.all(writes)).every(Boolean)) closeCustomizeWebapp()
    } finally {
      savingRef.current = false
      setSaving(false)
    }
  }

  const reset = async () => {
    if (savingRef.current) return
    if (!bootstrap) {
      toast.error("Instance settings aren't loaded yet — try again in a moment.")
      return
    }
    savingRef.current = true
    setSaving(true)
    try {
      const writes = [setInstanceIdentity({ title: "", favicon: "" })]
      // The config default is visible.
      if (!livePaneVisible) writes.push(setChangesPaneVisibility(true))
      if ((await Promise.all(writes)).every(Boolean)) closeCustomizeWebapp()
    } finally {
      savingRef.current = false
      setSaving(false)
    }
  }

  return (
    <DialogContent>
      <DialogHeader>
        <DialogTitle>Customize this webapp</DialogTitle>
        <DialogDescription>
          Set the browser tab title, favicon colour, and whether the Changes
          pane shows. Saved to config and applied to every connected browser.
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

      <div className="flex items-center gap-2">
        <Checkbox
          id="customize-show-changes"
          checked={showChanges}
          onCheckedChange={setPaneChoice}
        />
        <label htmlFor="customize-show-changes" className="text-sm">
          Show the Changes pane (the desktop layout's git panel)
        </label>
      </div>

      {/* Misclick-safe spacing between the checkbox and the footer buttons. */}
      <div className="h-2" />
      <DialogFooter className="sm:justify-between">
        <Button variant="ghost" disabled={saving} onClick={reset}>
          Reset to default
        </Button>
        <div className="flex flex-col-reverse gap-2 sm:flex-row">
          {/* Cancel is disabled too while a write is in flight: closing the
              dialog mid-persist would let the pending success close a freshly
              reopened dialog session, or misread as "the edit was discarded"
              when the request still lands on the server. */}
          <Button
            variant="outline"
            autoFocus
            disabled={saving}
            onClick={closeCustomizeWebapp}
          >
            Cancel
          </Button>
          <Button disabled={saving} onClick={save}>
            Save
          </Button>
        </div>
      </DialogFooter>
    </DialogContent>
  )
}

export function CustomizeWebappDialog() {
  const { customizeWebappOpen } = useDux()

  // One persist at a time. The ref gates re-entry synchronously (a double
  // click or a held Enter fires before React re-renders the disabled state);
  // the state disables the footer buttons, mirroring EditorOverlay's isSaving
  // convention. The guard lives HERE, not in the form, so onOpenChange can
  // also ignore Escape, backdrop clicks, and the header X while a write is in
  // flight — otherwise a mid-save dismissal unmounts the form and the orphaned
  // write's delayed success would close a freshly reopened dialog session.
  // Both clear in `finally`, so a failed save re-enables every dismiss path.
  const savingRef = useRef(false)
  const [saving, setSaving] = useState(false)

  return (
    <Dialog
      open={customizeWebappOpen}
      onOpenChange={(o) => {
        if (!o && !savingRef.current) closeCustomizeWebapp()
      }}
    >
      {customizeWebappOpen && (
        <CustomizeWebappForm
          saving={saving}
          setSaving={setSaving}
          savingRef={savingRef}
        />
      )}
    </Dialog>
  )
}
