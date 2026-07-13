import { useRef, useState } from "react"
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
import { ScrollArea } from "@/components/ui/scroll-area"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Switch } from "@/components/ui/switch"
import {
  DEFAULT_FAVICON_HREF,
  FAVICON_COLORS,
  duckFaviconDataUri,
} from "@/lib/favicon"
import {
  SETTING_GROUPS,
  allSettingDescriptors,
  type SettingDescriptor,
  type SettingValue,
} from "@/lib/settingsDescriptors"
import {
  closeCustomizeWebapp,
  saveSettings,
  setInstanceIdentity,
  useDux,
} from "@/lib/store"
import { cn } from "@/lib/utils"
import type { Bootstrap } from "@/lib/bootstrapApi"

// The favicon swatch grid. "Original" (favicon "") previews the bundled
// full-colour duck; each curated colour previews the tinted duck silhouette.
const SWATCHES: { value: string; label: string; href: string }[] = [
  { value: "", label: "Original", href: DEFAULT_FAVICON_HREF },
  ...Object.entries(FAVICON_COLORS).map(([name, hex]) => ({
    value: name,
    label: name.charAt(0).toUpperCase() + name.slice(1),
    href: duckFaviconDataUri(hex),
  })),
]

function rowId(d: SettingDescriptor): string {
  return `setting-${d.key.replace(/\./g, "-")}`
}

// Human copy for a descriptor's documented default, shown muted under the
// label so the row is self-explanatory without leaving the modal.
function defaultLabel(d: SettingDescriptor): string {
  switch (d.control.kind) {
    case "bool":
      return `Default: ${d.default ? "On" : "Off"}`
    case "number": {
      const unit = d.control.unit ? ` ${d.control.unit}` : ""
      return `Default: ${d.default}${unit}`
    }
    case "enum": {
      const opt = d.control.options.find((o) => o.value === d.default)
      return `Default: ${opt?.label ?? String(d.default)}`
    }
    case "text":
      return d.default ? `Default: ${d.default}` : "Default: empty"
    case "favicon":
      return "Default: Original"
  }
}

function FaviconControl({
  value,
  onChange,
  disabled,
}: {
  value: string
  onChange: (v: string) => void
  disabled: boolean
}) {
  return (
    <div className="grid grid-cols-6 gap-2">
      {SWATCHES.map((swatch) => {
        const selected = swatch.value === value
        return (
          <SimpleTooltip key={swatch.value || "original"} content={swatch.label}>
            <button
              type="button"
              aria-label={swatch.label}
              aria-pressed={selected}
              disabled={disabled}
              onClick={() => onChange(swatch.value)}
              className={cn(
                "relative flex aspect-square items-center justify-center rounded-lg border bg-muted/40 p-1.5 transition-colors max-md:min-h-10 hover:bg-muted disabled:opacity-50",
                selected ? "border-ring ring-3 ring-ring/50" : "border-input",
              )}
            >
              <img src={swatch.href} alt="" className="size-full object-contain" />
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
  )
}

function SettingControl({
  d,
  value,
  onChange,
  disabled,
}: {
  d: SettingDescriptor
  value: SettingValue
  onChange: (v: SettingValue) => void
  disabled: boolean
}) {
  const id = rowId(d)
  switch (d.control.kind) {
    case "bool":
      return (
        <Switch
          id={id}
          aria-label={d.label}
          checked={value as boolean}
          onCheckedChange={onChange}
          disabled={disabled}
        />
      )
    case "number":
      return (
        <Input
          id={id}
          type="number"
          aria-label={d.label}
          min={d.control.min}
          max={d.control.max}
          value={value as number}
          disabled={disabled}
          className="max-md:min-h-10 w-24"
          onChange={(e) => {
            const raw = Number(e.target.value)
            onChange(Number.isFinite(raw) ? raw : 0)
          }}
        />
      )
    case "enum":
      return (
        <Select
          value={value as string}
          onValueChange={(v) => onChange(v as string)}
          disabled={disabled}
        >
          <SelectTrigger id={id} aria-label={d.label} className="max-md:min-h-10">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {d.control.options.map((o) => (
              <SelectItem key={o.value} value={o.value}>
                {o.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )
    case "text":
      return (
        <Input
          id={id}
          aria-label={d.label}
          value={value as string}
          maxLength={d.control.maxLen}
          placeholder={String(d.default)}
          disabled={disabled}
          className="max-md:min-h-10"
          onChange={(e) => onChange(e.target.value)}
        />
      )
    case "favicon":
      return (
        <FaviconControl
          value={value as string}
          onChange={onChange}
          disabled={disabled}
        />
      )
  }
}

function SettingRow({
  d,
  value,
  onChange,
  disabled,
}: {
  d: SettingDescriptor
  value: SettingValue
  onChange: (v: SettingValue) => void
  disabled: boolean
}) {
  const id = rowId(d)
  const labelEl =
    d.control.kind === "text" || d.control.kind === "favicon" ? (
      <label htmlFor={id} className="text-sm font-medium">
        {d.label}
      </label>
    ) : (
      <span id={`${id}-label`} className="text-sm font-medium">
        {d.label}
      </span>
    )
  return (
    <div className="flex flex-col gap-2 py-3 first:pt-0 md:flex-row md:items-start md:justify-between md:gap-6">
      <div className="flex flex-col gap-1">
        {labelEl}
        <p className="text-xs text-muted-foreground">{d.description}</p>
        <p className="text-xs text-muted-foreground">
          {defaultLabel(d)}
          {d.control.kind === "number" && d.control.zeroMeaning
            ? ` (0 = ${d.control.zeroMeaning})`
            : ""}
        </p>
      </div>
      <div className="shrink-0 md:pt-0.5">
        <SettingControl d={d} value={value} onChange={onChange} disabled={disabled} />
      </div>
    </div>
  )
}

// Build the write bodies for a set of [descriptor, value] pairs, keeping only
// entries whose value actually differs from the live bootstrap (so an
// untouched-but-equal field, and a field touched back to its original value,
// never gets sent). Returns null when there is nothing to write.
function buildWrites(
  entries: [SettingDescriptor, SettingValue][],
  bootstrap: Bootstrap,
): {
  identity: { title?: string; favicon?: string } | null
  settings: { ui?: Record<string, SettingValue>; capabilities?: Record<string, SettingValue> } | null
} {
  const identity: { title?: string; favicon?: string } = {}
  const ui: Record<string, SettingValue> = {}
  const capabilities: Record<string, SettingValue> = {}
  for (const [d, value] of entries) {
    if (value === d.read(bootstrap)) continue
    if (d.writeTarget === "identity") {
      const field = d.key.split(".")[1] as "title" | "favicon"
      identity[field] = value as string
    } else {
      const [group, field] = d.key.split(".")
      if (group === "capabilities") capabilities[field] = value
      else ui[field] = value
    }
  }
  return {
    identity: Object.keys(identity).length ? identity : null,
    settings:
      Object.keys(ui).length || Object.keys(capabilities).length
        ? {
            ui: Object.keys(ui).length ? ui : undefined,
            capabilities: Object.keys(capabilities).length ? capabilities : undefined,
          }
        : null,
  }
}

async function persist(
  entries: [SettingDescriptor, SettingValue][],
  bootstrap: Bootstrap,
): Promise<boolean> {
  const { identity, settings } = buildWrites(entries, bootstrap)
  const writes: Promise<boolean>[] = []
  if (identity) writes.push(setInstanceIdentity(identity))
  if (settings) writes.push(saveSettings(settings))
  if (writes.length === 0) return true
  return (await Promise.all(writes)).every(Boolean)
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
  // `overrides` holds only the fields the user has explicitly touched in this
  // dialog session. Every other row renders the LIVE bootstrap value, so a
  // change made by another connected client while this dialog is open is
  // reflected instead of silently reverted on Save (mirrors the pre-existing
  // Changes-pane tracking, generalized to every row).
  const [overrides, setOverrides] = useState<Record<string, SettingValue>>({})

  const effective = (d: SettingDescriptor): SettingValue => {
    if (d.key in overrides) return overrides[d.key]
    return bootstrap ? d.read(bootstrap) : d.default
  }
  const setOverride = (key: string, value: SettingValue) =>
    setOverrides((o) => ({ ...o, [key]: value }))

  // Refuse to write before the config is loaded. The form seeds its fields
  // from `bootstrap`, so saving with a null bootstrap would persist fallback
  // defaults over whatever the operator actually configured.
  const requireBootstrap = (): boolean => {
    if (bootstrap) return true
    toast.error("Instance settings aren't loaded yet, try again in a moment.")
    return false
  }

  const save = async () => {
    if (savingRef.current) return
    if (!requireBootstrap() || !bootstrap) return
    savingRef.current = true
    setSaving(true)
    try {
      const touched: [SettingDescriptor, SettingValue][] = allSettingDescriptors()
        .filter((d) => d.key in overrides)
        .map((d) => [d, overrides[d.key]])
      if (await persist(touched, bootstrap)) closeCustomizeWebapp()
    } finally {
      savingRef.current = false
      setSaving(false)
    }
  }

  const resetSection = async (settings: SettingDescriptor[]) => {
    if (savingRef.current) return
    if (!requireBootstrap() || !bootstrap) return
    savingRef.current = true
    setSaving(true)
    try {
      // Identity fields (title/favicon) reset to an EMPTY string, not the
      // literal default text. The server's normalizer resolves an empty
      // title to "dux" and an empty favicon to the bundled duck, mirroring
      // the pre-existing "Reset to default" behavior for those two fields.
      const resetValue = (d: SettingDescriptor): SettingValue =>
        d.writeTarget === "identity" ? "" : d.default
      const entries: [SettingDescriptor, SettingValue][] = settings.map((d) => [
        d,
        resetValue(d),
      ])
      setOverrides((o) => {
        const next = { ...o }
        for (const d of settings) next[d.key] = resetValue(d)
        return next
      })
      await persist(entries, bootstrap)
    } finally {
      savingRef.current = false
      setSaving(false)
    }
  }

  return (
    <DialogContent className="max-h-[85vh] sm:max-w-2xl">
      <DialogHeader>
        <DialogTitle>Settings</DialogTitle>
        <DialogDescription>
          Configure dux. Saved to config.toml and applied to every connected
          browser.
        </DialogDescription>
      </DialogHeader>

      <ScrollArea className="max-h-[60vh] pr-3">
        <div className="flex flex-col gap-6">
          {SETTING_GROUPS.map((group) => (
            <div key={group.surface} className="flex flex-col gap-1">
              <div className="flex items-center justify-between gap-3">
                <p className="text-xs font-medium text-muted-foreground">
                  {group.caption}
                </p>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  disabled={saving}
                  className="max-md:min-h-10 shrink-0 text-xs"
                  onClick={() => resetSection(group.settings)}
                >
                  Reset section to defaults…
                </Button>
              </div>
              <div className="divide-y divide-border">
                {group.settings.map((d) => (
                  <SettingRow
                    key={d.key}
                    d={d}
                    value={effective(d)}
                    onChange={(v) => setOverride(d.key, v)}
                    disabled={saving}
                  />
                ))}
              </div>
            </div>
          ))}
        </div>
      </ScrollArea>

      {/* Misclick-safe spacing between the last row and the footer buttons. */}
      <div className="h-2" />
      <DialogFooter>
        <div className="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
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
  // the state disables the footer buttons. The guard lives HERE, not in the
  // form, so onOpenChange can also ignore Escape, backdrop clicks, and the
  // header X while a write is in flight, otherwise a mid-save dismissal
  // unmounts the form and the orphaned write's delayed success would close a
  // freshly reopened dialog session. Both clear in `finally`, so a failed
  // save re-enables every dismiss path.
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
