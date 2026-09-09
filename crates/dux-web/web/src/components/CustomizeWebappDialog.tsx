import { Fragment, useRef, useState } from "react"
import { Check } from "lucide-react"
import {
  notifyError,
  notifyInfo,
  notifySuccess,
  notifyWarning,
} from "@/lib/notify"

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
import { configApi } from "@/lib/configApi"
import { renderInlineCode } from "@/lib/inlineMarkdown"
import {
  changesPaneVisible,
  closeCustomizeWebapp,
  mobileAccessoryBarVisible,
  saveSettings,
  setChangesPaneVisibility,
  setInstanceIdentity,
  useDux,
} from "@/lib/store"
import { cn } from "@/lib/utils"

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
    case "enum-dynamic":
      return `Default: ${d.default}`
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
    <div className="flex max-w-[15.5rem] flex-wrap gap-2">
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
                "relative flex size-10 shrink-0 items-center justify-center rounded-lg border bg-muted/40 p-1.5 transition-colors hover:bg-muted disabled:opacity-50",
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

// Clamps to the nearer bound WHILE TYPING, so a number field never rejects a
// keystroke mid-edit. An out-of-range configured value degrades to the documented
// default instead (`clampTerminalFontSize`); the two answer different questions.
function clampToControl(n: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, n))
}

// Local text preserves an empty in-progress field without committing it as zero.
// Finite values commit within bounds; external value changes resync during render.
function NumberControl({
  id,
  label,
  min,
  max,
  value,
  onChange,
  disabled,
}: {
  id: string
  label: string
  min: number
  max: number
  value: number
  onChange: (v: number) => void
  disabled: boolean
}) {
  const [text, setText] = useState(String(value))
  const [prevValue, setPrevValue] = useState(value)
  if (value !== prevValue) {
    setPrevValue(value)
    setText(String(value))
  }

  return (
    <Input
      id={id}
      type="number"
      aria-label={label}
      min={min}
      max={max}
      value={text}
      disabled={disabled}
      className="max-md:min-h-10 w-24"
      onChange={(e) => {
        const raw = e.target.value
        setText(raw)
        if (raw.trim() === "") return
        const parsed = Number(raw)
        if (!Number.isFinite(parsed)) return
        onChange(clampToControl(parsed, min, max))
      }}
      onBlur={() => {
        // Leaving the field empty reverts the displayed text to the last committed
        // value, rather than stranding a blank input over a stale override.
        if (text.trim() === "") setText(String(value))
      }}
    />
  )
}

function SettingControl({
  d,
  value,
  onChange,
  disabled,
  availableProviders,
}: {
  d: SettingDescriptor
  value: SettingValue
  onChange: (v: SettingValue) => void
  disabled: boolean
  availableProviders: string[]
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
        <NumberControl
          id={id}
          label={d.label}
          min={d.control.min}
          max={d.control.max}
          value={value as number}
          disabled={disabled}
          onChange={(v) => onChange(v)}
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
    case "enum-dynamic": {
      // Only "available_providers" exists today; the source tag is kept for
      // forward-compatibility with a future dynamic-option field.
      const options = availableProviders
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
            {options.map((p) => (
              <SelectItem key={p} value={p}>
                {p}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )
    }
    case "text":
      return (
        <Input
          id={id}
          aria-label={d.label}
          value={value as string}
          maxLength={d.control.maxLen}
          placeholder={String(d.default)}
          disabled={disabled}
          className="max-md:min-h-10 w-full md:w-56"
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

// Browser-notification permission, not a SettingDescriptor: it writes no config
// and asks the browser, per visitor. dux never auto-prompts, so this is the only
// way to grant it, and it shows only while permission is still "default".
function NotificationPermissionRow({ enabledInConfig }: { enabledInConfig: boolean }) {
  const apiAvailable = typeof Notification !== "undefined"
  const [permission, setPermission] = useState<NotificationPermission>(
    apiAvailable ? Notification.permission : "denied",
  )

  if (!enabledInConfig || !apiAvailable || permission !== "default") return null

  const request = async () => {
    try {
      const result = await Notification.requestPermission()
      setPermission(result)
      if (result === "granted") {
        notifySuccess("Browser notifications enabled for dux.")
      } else {
        notifyInfo("Browser notifications were not granted.")
      }
    } catch {
      notifyError("Could not request notification permission.")
    }
  }

  return (
    <div className="flex flex-col gap-2 py-3 first:pt-0 md:flex-row md:items-start md:justify-between md:gap-6">
      <div className="flex flex-col gap-1">
        <span className="text-sm font-medium">Browser permission</span>
        <p className="text-xs text-muted-foreground">
          This browser hasn&rsquo;t granted dux permission to show notifications
          yet, so the setting above can&rsquo;t do anything. dux never asks on its
          own, so grant it here when you want it.
        </p>
      </div>
      <div className="shrink-0 md:pt-0.5">
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="max-md:min-h-10"
          onClick={() => void request()}
        >
          Enable browser notifications
        </Button>
      </div>
    </div>
  )
}

function SettingRow({
  d,
  value,
  onChange,
  disabled,
  lock,
  availableProviders,
}: {
  d: SettingDescriptor
  value: SettingValue
  onChange: (v: SettingValue) => void
  disabled: boolean
  /** The sentence saying why this run cannot change the row, or null. */
  lock: string | null
  availableProviders: string[]
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
        <p className="text-xs text-muted-foreground">
          {renderInlineCode(lock ?? d.description)}
        </p>
        <p className="text-xs text-muted-foreground">
          {defaultLabel(d)}
          {d.control.kind === "number" && d.control.zeroMeaning
            ? ` (0 = ${d.control.zeroMeaning})`
            : ""}
        </p>
      </div>
      <div className="shrink-0 md:pt-0.5">
        <SettingControl
          d={d}
          value={value}
          onChange={onChange}
          disabled={disabled || lock !== null}
          availableProviders={availableProviders}
        />
      </div>
    </div>
  )
}

// Build the write bodies for [descriptor, value] pairs, sending only entries that
// differ from their pre-touch value; null when there is nothing to write.
// `originalOf` supplies that baseline and must be override-aware for any field
// something outside this dialog can flip live, or a toggle back to a stale
// bootstrap value reads as a no-op. See `writeTarget` in `settingsDescriptors.ts`.
function buildWrites(
  entries: [SettingDescriptor, SettingValue][],
  originalOf: (d: SettingDescriptor) => SettingValue,
): {
  identity: { title?: string; favicon?: string } | null
  settings: {
    ui?: Record<string, SettingValue>
    capabilities?: Record<string, SettingValue>
    defaults?: Record<string, SettingValue>
  } | null
  changesPane: boolean | null
  github: boolean | null
  tailscale: string | null
} {
  const identity: { title?: string; favicon?: string } = {}
  const ui: Record<string, SettingValue> = {}
  const capabilities: Record<string, SettingValue> = {}
  const defaults: Record<string, SettingValue> = {}
  let changesPane: boolean | null = null
  let tailscale: string | null = null
  let github: boolean | null = null
  for (const [d, value] of entries) {
    // The unchanged-row skip. The `github` target posts to a blind read-and-FLIP
    // endpoint, so emitting an unchanged row would invert the setting.
    if (value === originalOf(d)) continue
    if (d.writeTarget === "identity") {
      const field = d.key.split(".")[1] as "title" | "favicon"
      identity[field] = value as string
    } else if (d.writeTarget === "changesPane") {
      changesPane = value as boolean
    } else if (d.writeTarget === "github") {
      github = value as boolean
    } else if (d.writeTarget === "tailscale") {
      tailscale = value as string
    } else {
      const [group, field] = d.key.split(".")
      // The single flip point for an `inverted` row: everything upstream, the seed,
      // the switch and the unchanged-row skip, works in shown values only.
      const wire = d.inverted ? !(value as boolean) : value
      if (group === "capabilities") capabilities[field] = wire
      else if (group === "defaults") defaults[field] = wire
      else ui[field] = wire
    }
  }
  const hasSettings =
    Object.keys(ui).length ||
    Object.keys(capabilities).length ||
    Object.keys(defaults).length
  return {
    identity: Object.keys(identity).length ? identity : null,
    settings: hasSettings
      ? {
          ui: Object.keys(ui).length ? ui : undefined,
          capabilities: Object.keys(capabilities).length ? capabilities : undefined,
          defaults: Object.keys(defaults).length ? defaults : undefined,
        }
      : null,
    changesPane,
    github,
    tailscale,
  }
}

async function persist(
  entries: [SettingDescriptor, SettingValue][],
  originalOf: (d: SettingDescriptor) => SettingValue,
): Promise<boolean> {
  const { identity, settings, changesPane, github, tailscale } = buildWrites(
    entries,
    originalOf,
  )
  const writes: Promise<boolean>[] = []
  if (identity) writes.push(setInstanceIdentity(identity))
  if (settings) writes.push(saveSettings(settings))
  if (changesPane !== null) writes.push(setChangesPaneVisibility(changesPane))
  // `github` is non-null ONLY when the row changed, which is what makes it safe
  // to drive a read-and-flip endpoint from an explicit-value UI.
  if (github !== null) {
    writes.push(
      configApi
        .toggleGithubIntegration()
        .then(() => true)
        .catch((e) => {
          notifyError(
            e instanceof Error ? e.message : "Could not toggle GitHub integration.",
          )
          return false
        }),
    )
  }
  // The one row whose reply is a sentence rather than a status code, raised even
  // on success: saved and "the listener went away" are different outcomes.
  if (tailscale !== null) {
    const mode = tailscale
    writes.push(
      configApi
        .setTailscaleMode(mode)
        .then((reply) => {
          if (reply.warning) notifyWarning(reply.message)
          else notifyInfo(reply.message)
          return true
        })
        .catch((e) => {
          notifyError(
            e instanceof Error
              ? e.message
              : "Could not change the Tailscale mode.",
          )
          return false
        }),
    )
  }
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
  // `overrides` holds only the fields touched in this dialog session; every other
  // row renders live bootstrap, so another client's change is not reverted on Save.
  const [overrides, setOverrides] = useState<Record<string, SettingValue>>({})

  // The pre-touch baseline for a row. Any field the store tracks an optimistic
  // override for must read the override-aware selector rather than raw bootstrap:
  // the rule is override-awareness, not `writeTarget`.
  const originalOf = (d: SettingDescriptor): SettingValue => {
    if (d.writeTarget === "changesPane") return changesPaneVisible(dux)
    if (d.key === "ui.mobile_accessory_bar") return mobileAccessoryBarVisible(dux)
    return bootstrap ? d.read(bootstrap) : d.default
  }

  const effective = (d: SettingDescriptor): SettingValue => {
    if (d.key in overrides) return overrides[d.key]
    return originalOf(d)
  }
  const setOverride = (key: string, value: SettingValue) =>
    setOverrides((o) => ({ ...o, [key]: value }))

  // What this RUN of the server refuses to let the row change, if anything.
  const lockOn = (d: SettingDescriptor): string | null =>
    bootstrap ? (d.lockedBy?.(bootstrap) ?? null) : null

  // Refuse to write before the config is loaded: the form seeds from `bootstrap`,
  // so a null one would persist fallback defaults over the real configuration.
  const requireBootstrap = (): boolean => {
    if (bootstrap) return true
    notifyError("Instance settings aren't loaded yet, try again in a moment.")
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
      if (await persist(touched, originalOf)) closeCustomizeWebapp()
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
      // Identity fields reset to an EMPTY string, not the literal default text:
      // the server's normalizer resolves empty to the shipped title and favicon.
      const resetValue = (d: SettingDescriptor): SettingValue =>
        d.writeTarget === "identity" ? "" : d.default
      // A locked row's control is unreachable, and a reset must not write past it:
      // the run would refuse the value and the toast would contradict the dialog.
      const entries: [SettingDescriptor, SettingValue][] = settings
        .filter((d) => lockOn(d) === null)
        .map((d) => [d, resetValue(d)])
      // Reflect the reset defaults only AFTER the write lands: an optimistic
      // override would show a failed reset as saved. The dialog stays open either way.
      if (await persist(entries, originalOf)) {
        setOverrides((o) => {
          const next = { ...o }
          for (const [d, value] of entries) next[d.key] = value
          return next
        })
      }
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
          {renderInlineCode(
            "Configure dux. Saved to `config.toml` and applied to every connected browser.",
          )}
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
                  <Fragment key={d.key}>
                    <SettingRow
                      d={d}
                      value={effective(d)}
                      onChange={(v) => setOverride(d.key, v)}
                      disabled={saving}
                      lock={lockOn(d)}
                      availableProviders={bootstrap?.available_providers ?? []}
                    />
                    {/* Directly beneath the setting it is a precondition for. */}
                    {d.key === "capabilities.web_notifications" ? (
                      <NotificationPermissionRow
                        enabledInConfig={effective(d) as boolean}
                      />
                    ) : null}
                  </Fragment>
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

  // One persist at a time: the ref gates re-entry synchronously, the state disables
  // the footer, and both gate `onOpenChange` too, so no dismissal orphans a write.
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
