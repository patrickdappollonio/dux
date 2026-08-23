import { Fragment, useRef, useState } from "react"
import { Check } from "lucide-react"
import { notifyError, notifyInfo, notifySuccess } from "@/lib/notify"

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
  mobileTopBarVisible,
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

// Clamps to the nearer bound WHILE THE USER IS TYPING: an input affordance so
// a number field never rejects a keystroke mid-edit. This is deliberately
// different from how an out-of-range value from config/bootstrap is handled
// once it lands in the running app: `clampTerminalFontSize` in
// `lib/terminalFont.ts` (and its server-side twin,
// `normalized_terminal_font_size` in `crates/dux-core/src/config.rs`) DEGRADE
// such a value to the documented default instead of nudging it to the nearest
// bound, so a value that is merely wrong reads as an obviously-reset default.
// Both behaviors are intentional; they simply answer different questions.
function clampToControl(n: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, n))
}

// A number row keeps its OWN local text buffer, decoupled from the committed
// `value` prop, for two reasons (see the adversarial-review findings this
// fixes): (1) an emptied field must render empty while the user is between
// keystrokes, and Input's `value` is otherwise fully controlled by the
// committed numeric override, so without a local buffer the field would snap
// back to a stale digit the instant the last character is deleted; (2) an
// empty field must NOT commit as `0`, since 0 is a meaningful value for every
// numeric setting here (never auto-clear, clear immediately, leave tabs
// as-is). Every keystroke that parses to a finite number commits immediately,
// clamped to the descriptor's [min, max] client-side so an out-of-range value
// never reaches the wire (the server re-clamps too, but silently, which is
// surprising UX on its own). The buffer re-syncs whenever the committed
// `value` changes from OUTSIDE this control (an override reset, or a
// concurrent client's bootstrap update reflected while untouched), via the
// React-documented "adjust state during render" pattern (a `prevValue`
// tracker compared on every render) rather than a `useEffect`, so the
// re-sync lands in the SAME render/commit as the prop change instead of a
// cascading extra render.
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
        // Leaving the field empty (or on some other non-committing state)
        // reverts the displayed text to the last committed value rather than
        // stranding a blank input with a stale numeric override underneath.
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

// Browser-notification permission. NOT a SettingDescriptor: it writes no config,
// it asks the BROWSER for permission, which is per-visitor and per-browser state
// the server never sees. It sits directly under the "Desktop notifications"
// (`capabilities.web_notifications`) row because that setting cannot do anything
// until this permission is granted, so the setting and its precondition belong
// together.
//
// This affordance is the ONLY way to grant permission: dux deliberately never
// auto-prompts, so the visitor must opt in explicitly. The row appears only while there is
// something to ask for (notifications enabled in config, the API exists, and
// permission is still "default"); once granted or denied, the browser owns the
// decision and dux cannot re-ask.
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
  availableProviders,
}: {
  d: SettingDescriptor
  value: SettingValue
  onChange: (v: SettingValue) => void
  disabled: boolean
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
        <p className="text-xs text-muted-foreground">{renderInlineCode(d.description)}</p>
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
          disabled={disabled}
          availableProviders={availableProviders}
        />
      </div>
    </div>
  )
}

// Build the write bodies for a set of [descriptor, value] pairs, keeping only
// entries whose value actually differs from its ORIGINAL (pre-touch) value
// (so an untouched-but-equal field, and a field touched back to its original
// value, never gets sent). Returns null when there is nothing to write.
//
// `originalOf` supplies that pre-touch baseline. It is NOT always
// `d.read(bootstrap)`: `ui.show_changes_pane` (`writeTarget: "changesPane"`)
// is bespoke, the store tracks an optimistic `changesPaneOverride` for that
// one field (the Changes menu can flip it live outside this dialog too), so
// its baseline is the override-aware `changesPaneVisible()`, not the raw
// bootstrap field, and comparing against the raw field would wrongly treat a
// toggle back to the STALE bootstrap value as a no-op. The same
// override-awareness applies to the two mobile-bar rows (`ui.mobile_top_bar`,
// `ui.mobile_accessory_bar`), which ride the generic PATCH but are flipped
// live by the terminal screen's quick toggles; the dialog's `originalOf`
// resolves all of these to the store's override-aware selectors. Separately,
// `ui.show_changes_pane` is excluded from the generic `settings` bucket here
// and routed through `setChangesPaneVisibility` by the caller instead of the
// generic PATCH. See the `writeTarget` doc in `settingsDescriptors.ts`.
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
} {
  const identity: { title?: string; favicon?: string } = {}
  const ui: Record<string, SettingValue> = {}
  const capabilities: Record<string, SettingValue> = {}
  const defaults: Record<string, SettingValue> = {}
  let changesPane: boolean | null = null
  let github: boolean | null = null
  for (const [d, value] of entries) {
    // THE unchanged-row skip. Load-bearing well beyond avoiding a redundant
    // write: the `github` target below posts to a blind read-and-FLIP endpoint,
    // so emitting an unchanged row would invert the setting. See the
    // `writeTarget` doc in settingsDescriptors.ts.
    if (value === originalOf(d)) continue
    if (d.writeTarget === "identity") {
      const field = d.key.split(".")[1] as "title" | "favicon"
      identity[field] = value as string
    } else if (d.writeTarget === "changesPane") {
      changesPane = value as boolean
    } else if (d.writeTarget === "github") {
      github = value as boolean
    } else {
      const [group, field] = d.key.split(".")
      // THE single flip point for an `inverted` row (see the descriptor's doc):
      // the row shows "Show the welcome screen", the config field is
      // `disable_automated_welcome_screen`. Everything upstream — the seed, the
      // switch, the unchanged-row skip above — works in shown-values only.
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
  }
}

async function persist(
  entries: [SettingDescriptor, SettingValue][],
  originalOf: (d: SettingDescriptor) => SettingValue,
): Promise<boolean> {
  const { identity, settings, changesPane, github } = buildWrites(
    entries,
    originalOf,
  )
  const writes: Promise<boolean>[] = []
  if (identity) writes.push(setInstanceIdentity(identity))
  if (settings) writes.push(saveSettings(settings))
  if (changesPane !== null) writes.push(setChangesPaneVisibility(changesPane))
  // `github` is non-null ONLY when the row actually changed, which is what makes
  // it safe to drive a read-and-flip endpoint from an explicit-value UI. Do NOT
  // "simplify" this to write unconditionally.
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

  // The pre-touch baseline for a row: what it would show/compare against if
  // the user had never touched it in this dialog session. The rule for the
  // special cases is OVERRIDE-AWARENESS, not writeTarget: any field the store
  // tracks an optimistic override for (something outside this dialog can flip
  // it live — the Changes menu, the mobile terminal screen's quick toggles,
  // the input ⋯ menu below the terminal) must read the store's override-aware
  // selector, not the raw bootstrap field. Reading the raw field would show a
  // stale value until the next bootstrap refetch reconciles it, and would
  // make `buildWrites` wrongly treat a toggle back to that stale value as a
  // no-op. That the Changes-pane case coincides with a bespoke writeTarget is
  // incidental; the two mobile-bar rows ride the generic settings PATCH and
  // still need their baseline read this way.
  const originalOf = (d: SettingDescriptor): SettingValue => {
    if (d.writeTarget === "changesPane") return changesPaneVisible(dux)
    if (d.key === "ui.mobile_top_bar") return mobileTopBarVisible(dux)
    if (d.key === "ui.mobile_accessory_bar") return mobileAccessoryBarVisible(dux)
    return bootstrap ? d.read(bootstrap) : d.default
  }

  const effective = (d: SettingDescriptor): SettingValue => {
    if (d.key in overrides) return overrides[d.key]
    return originalOf(d)
  }
  const setOverride = (key: string, value: SettingValue) =>
    setOverrides((o) => ({ ...o, [key]: value }))

  // Refuse to write before the config is loaded. The form seeds its fields
  // from `bootstrap`, so saving with a null bootstrap would persist fallback
  // defaults over whatever the operator actually configured.
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
      // Only reflect the reset defaults in the dialog's local state AFTER the
      // write actually lands. Applying the optimistic override first (the
      // previous behavior) showed reset defaults in the controls even when
      // `persist()` failed, misrepresenting an unsaved state as saved. The
      // dialog stays OPEN either way, "reset section" is a section-scoped
      // action the user may keep editing after, not a full-dialog commit
      // like Save, so a failure just leaves the prior values in place for
      // the user to see the error toast and retry.
      if (await persist(entries, originalOf)) {
        setOverrides((o) => {
          const next = { ...o }
          for (const d of settings) next[d.key] = resetValue(d)
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
