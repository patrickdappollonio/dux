import { Fragment, useEffect, useRef, useState } from "react"
import { Check, Search, ZoomOut } from "lucide-react"
import {
  notifyError,
  notifyInfo,
  notifySuccess,
  notifyWarning,
} from "@/lib/notify"

import { MatchHighlight } from "@/components/MatchHighlight"
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
import { InlineCode } from "@/components/ui/inline-code"
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
import type { AuthStatus } from "@/lib/authApi"
import {
  changePassword,
  refreshAuthStatus,
  reportUnauthorized,
  useAuthPhase,
} from "@/lib/authGate"
import { EDIT_CONFIG_TITLE } from "@/lib/appMenu"
import { configApi } from "@/lib/configApi"
import {
  STRENGTH_LABELS,
  estimateStrength,
  passwordMinimums,
  passwordWrite,
  type PasswordDraft,
  type Strength,
} from "@/lib/passwordStrength"
import { firstPasswordUnavailable, storedNotInForceSentence } from "@/lib/authErrors"
import { inlineCodeDisplayText, renderInlineCode } from "@/lib/inlineMarkdown"
import { haystackHas, normalizeQuery } from "@/lib/agentSearch"
import { matchCharRanges } from "@/lib/searchMatch"
import { useIsCoarsePointer } from "@/hooks/use-coarse-pointer"
import { useIsMobile } from "@/hooks/use-mobile"
import { wireProse } from "@/lib/prose"
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
function defaultText(d: SettingDescriptor): string {
  const zero =
    d.control.kind === "number" && d.control.zeroMeaning
      ? ` (0 = ${d.control.zeroMeaning})`
      : ""
  return defaultLabel(d) + zero
}

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
    case "password":
      return "Default: no password"
  }
}

// The Settings search splits its query into words, and a row is shown only
// when EVERY word is somewhere in what the row shows: its label, its prose, its
// default and its config key. The fields are the shown text, so a hit is
// always something the user can see lit.
function searchWords(query: string): string[] {
  return normalizeQuery(query).split(/\s+/).filter((w) => w !== "")
}

function rowMatches(fields: string[], words: string[]): boolean {
  return words.every((word) => haystackHas(word, ...fields))
}

// What a row shows, as searched. A locked row shows its lock sentence in place
// of its description; the password row shows no default. `extra` is any other
// sentence the row shows, in its backticked source form.
function settingSearchFields(
  d: SettingDescriptor,
  lock: string | null,
  extra: string[],
): string[] {
  const fields = [
    d.label,
    inlineCodeDisplayText(lock ?? d.description),
    d.key,
    ...extra.map(inlineCodeDisplayText),
  ]
  if (d.control.kind !== "password") fields.push(defaultText(d))
  return fields
}

// The row's label, lit where the search matched it, and while a search is
// running its config key on a line of its own, so a key typed from
// `config.toml` finds the row and shows why.
function RowHeading({
  d,
  words,
  label,
}: {
  d: SettingDescriptor
  words: string[]
  label: React.ReactNode
}) {
  return (
    <>
      {label}
      {words.length > 0 ? (
        <span className="font-mono text-xs break-all text-muted-foreground">
          <MatchHighlight text={d.key} ranges={matchCharRanges(d.key, words)} />
        </span>
      ) : null}
    </>
  )
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
    case "password":
      // Rendered by `PasswordSettingRow`, never through the generic row.
      return null
  }
}

// Browser-notification permission, not a SettingDescriptor: it writes no config
// and asks the browser, per visitor. dux never auto-prompts, so this is the only
// way to grant it, and it shows only while permission is still "default". It
// sits under the desktop-notifications row and is searched as part of it.
const NOTIFICATION_PERMISSION_TEXT = {
  title: "Browser permission",
  body: "This browser hasn’t granted dux permission to show notifications yet, so the setting above can’t do anything. dux never asks on its own, so grant it here when you want it.",
  action: "Enable browser notifications",
}

function initialNotificationPermission(): NotificationPermission {
  return typeof Notification !== "undefined" ? Notification.permission : "denied"
}

function notificationPermissionShown(
  enabledInConfig: boolean,
  permission: NotificationPermission,
): boolean {
  return enabledInConfig && typeof Notification !== "undefined" && permission === "default"
}

function NotificationPermissionRow({
  setPermission,
  words,
}: {
  setPermission: (p: NotificationPermission) => void
  words: string[]
}) {
  const lit = (text: string) => (
    <MatchHighlight text={text} ranges={matchCharRanges(text, words)} />
  )

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
        <span className="text-sm font-medium">{lit(NOTIFICATION_PERMISSION_TEXT.title)}</span>
        <p className="text-xs text-muted-foreground">{lit(NOTIFICATION_PERMISSION_TEXT.body)}</p>
      </div>
      <div className="shrink-0 md:pt-0.5">
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="max-md:min-h-10"
          onClick={() => void request()}
        >
          {lit(NOTIFICATION_PERMISSION_TEXT.action)}
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
  words,
}: {
  d: SettingDescriptor
  value: SettingValue
  onChange: (v: SettingValue) => void
  disabled: boolean
  /** The sentence saying why this run cannot change the row, or null. */
  lock: string | null
  availableProviders: string[]
  /** The search's words, lit wherever they occur; empty when not searching. */
  words: string[]
}) {
  const id = rowId(d)
  const labelText = <MatchHighlight text={d.label} ranges={matchCharRanges(d.label, words)} />
  const labelEl =
    d.control.kind === "text" || d.control.kind === "favicon" ? (
      <label htmlFor={id} className="text-sm font-medium">
        {labelText}
      </label>
    ) : (
      <span id={`${id}-label`} className="text-sm font-medium">
        {labelText}
      </span>
    )
  const shownDefault = defaultText(d)
  return (
    <div className="flex flex-col gap-2 py-3 first:pt-0 md:flex-row md:items-start md:justify-between md:gap-6">
      <div className="flex flex-col gap-1">
        <RowHeading d={d} words={words} label={labelEl} />
        <p className="text-xs text-muted-foreground">
          {renderInlineCode(lock ?? d.description, words)}
        </p>
        <p className="text-xs text-muted-foreground">
          <MatchHighlight text={shownDefault} ranges={matchCharRanges(shownDefault, words)} />
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

const EMPTY_PASSWORD_DRAFT: PasswordDraft = { current: "", next: "", confirm: "" }

interface StrengthReading {
  /** The meter's reading, or null while it is computed or when it failed. */
  strength: Strength | null
  /** The dictionaries did not load, so there is no reading to show. */
  failed: boolean
  /** Load them again. */
  retry: () => void
}

// The meter's reading of `password`. Keyed by the password it was computed for,
// so a slow answer for an earlier keystroke is never shown against a later one.
// Advisory only: nothing here can hold up a Save.
function usePasswordStrength(password: string): StrengthReading {
  const [reading, setReading] = useState<{
    password: string
    attempt: number
    strength: Strength | null
  } | null>(null)
  const [attempt, setAttempt] = useState(0)
  useEffect(() => {
    if (password === "") return
    let live = true
    estimateStrength(password).then(
      (strength) => {
        if (live) setReading({ password, attempt, strength })
      },
      () => {
        if (live) setReading({ password, attempt, strength: null })
      },
    )
    return () => {
      live = false
    }
  }, [password, attempt])
  const current = reading !== null && reading.password === password && reading.attempt === attempt
  return {
    strength: current ? reading.strength : null,
    failed: current && reading.strength === null,
    retry: () => setAttempt((n) => n + 1),
  }
}

function StrengthMeter({ strength, minScore }: { strength: Strength; minScore: number }) {
  const below = strength.score < minScore
  return (
    <div className="flex flex-col gap-1">
      <div
        role="meter"
        aria-label="Password strength"
        aria-valuemin={0}
        aria-valuemax={4}
        aria-valuenow={strength.score}
        aria-valuetext={strength.label}
        className="flex gap-1"
      >
        {[0, 1, 2, 3, 4].map((i) => (
          <span
            key={i}
            className={cn(
              "h-1.5 flex-1 rounded-full",
              i <= strength.score ? (below ? "bg-destructive" : "bg-primary") : "bg-muted",
            )}
          />
        ))}
      </div>
      <p className="text-xs text-muted-foreground">
        Strength: <span className={cn("font-medium", below && "text-destructive")}>{strength.label}</span>
        {below
          ? `, below the ${STRENGTH_LABELS[Math.min(4, minScore)]} this dux asks for, so it will probably refuse it`
          : ""}
      </p>
      {strength.hint ? <p className="text-xs text-muted-foreground">{strength.hint}</p> : null}
    </div>
  )
}

// On a coarse pointer the fields meet the touch floor at any width; with a mouse
// they keep the shared input height, stacked a gap-3 apart.
const PASSWORD_FIELD_CLASS = "pointer-coarse:min-h-11 w-full md:w-64"

function PasswordField({
  id,
  label,
  value,
  autoComplete,
  disabled,
  onChange,
}: {
  id: string
  label: string
  value: string
  autoComplete: string
  disabled: boolean
  onChange: (v: string) => void
}) {
  return (
    <div className="flex flex-col gap-1">
      <label htmlFor={id} className="text-xs font-medium">
        {label}
      </label>
      <Input
        id={id}
        type="password"
        autoComplete={autoComplete}
        value={value}
        disabled={disabled}
        className={PASSWORD_FIELD_CLASS}
        onChange={(e) => onChange(e.target.value)}
      />
    </div>
  )
}

// The one row that is not a `SettingRow`: three fields, a meter and its own
// error line, all of it sent through the `"password"` write target by Save.
// Which fields show is the server's answer about this connection, read again
// each time Preferences opens: change (a password is set), set the first one
// (allowed from here), or neither.
// The sentence under the password row's description: the minimums while the
// password can be changed from here, otherwise why it cannot. Searched as part
// of the row.
function passwordNote(status: AuthStatus | null): string {
  if (status === null) {
    return "dux could not read its sign-in status, so the password cannot be changed from here right now. Close Preferences and open it again to ask once more."
  }
  if (status.password_set || status.can_set_first_password) {
    const mins = passwordMinimums(status)
    return `At least ${mins.length} characters. The meter shows how hard it is to guess; dux asks for ${STRENGTH_LABELS[Math.min(4, mins.score)]} or better and says so if it refuses. Leave these empty to keep the current password.`
  }
  return firstPasswordUnavailable(status.required_reason)
}

function PasswordSettingRow({
  d,
  status,
  draft,
  onDraft,
  reading,
  error,
  disabled,
  words,
}: {
  d: SettingDescriptor
  status: AuthStatus | null
  draft: PasswordDraft
  onDraft: (next: PasswordDraft) => void
  reading: StrengthReading
  error: string | null
  disabled: boolean
  words: string[]
}) {
  const id = rowId(d)
  const editable =
    status !== null && (status.password_set || status.can_set_first_password)
  const mins = status ? passwordMinimums(status) : null
  return (
    <div className="flex flex-col gap-3 py-3 first:pt-0">
      <div className="flex flex-col gap-1">
        <RowHeading
          d={d}
          words={words}
          label={
            <span id={`${id}-label`} className="text-sm font-medium">
              <MatchHighlight text={d.label} ranges={matchCharRanges(d.label, words)} />
            </span>
          }
        />
        <p className="text-xs text-muted-foreground">
          {renderInlineCode(d.description, words)}
        </p>
        <p className="text-xs text-muted-foreground">
          {renderInlineCode(passwordNote(status), words)}
        </p>
      </div>
      {editable && mins ? (
        <div className="flex flex-col gap-3">
          {status.password_set ? (
            <PasswordField
              id={`${id}-current`}
              label="Current password"
              value={draft.current}
              autoComplete="current-password"
              disabled={disabled}
              onChange={(current) => onDraft({ ...draft, current })}
            />
          ) : null}
          <PasswordField
            id={`${id}-new`}
            label="New password"
            value={draft.next}
            autoComplete="new-password"
            disabled={disabled}
            onChange={(next) => onDraft({ ...draft, next })}
          />
          {draft.next !== "" && reading.strength !== null ? (
            <div className="md:w-64">
              <StrengthMeter strength={reading.strength} minScore={mins.score} />
            </div>
          ) : null}
          {draft.next !== "" && reading.failed ? (
            <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
              <span>The strength meter did not load. Save still works.</span>
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="pointer-coarse:min-h-11"
                onClick={reading.retry}
              >
                Load the strength meter again
              </Button>
            </div>
          ) : null}
          <PasswordField
            id={`${id}-confirm`}
            label="New password again"
            value={draft.confirm}
            autoComplete="new-password"
            disabled={disabled}
            onChange={(confirm) => onDraft({ ...draft, confirm })}
          />
          {error ? (
            <p role="alert" className="text-xs text-destructive">
              {error}
            </p>
          ) : null}
        </div>
      ) : null}
    </div>
  )
}

type SettingsGroup = Record<string, SettingValue>

interface SettingsBody {
  ui?: SettingsGroup
  capabilities?: SettingsGroup
  defaults?: SettingsGroup
}

// The settings patch, or null when no group changed. An untouched group is
// omitted rather than sent empty, so the server writes only what the user moved.
function settingsBody(groups: Required<SettingsBody>): SettingsBody | null {
  const changed = (rows: SettingsGroup) =>
    Object.keys(rows).length ? rows : undefined
  const body: SettingsBody = {
    ui: changed(groups.ui),
    capabilities: changed(groups.capabilities),
    defaults: changed(groups.defaults),
  }
  if (!body.ui && !body.capabilities && !body.defaults) return null
  return body
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
  settings: SettingsBody | null
  changesPane: boolean | null
  github: boolean | null
  tailscale: string | null
} {
  const identity: { title?: string; favicon?: string } = {}
  const ui: SettingsGroup = {}
  const capabilities: SettingsGroup = {}
  const defaults: SettingsGroup = {}
  let changesPane: boolean | null = null
  let tailscale: string | null = null
  let github: boolean | null = null
  for (const [d, value] of entries) {
    // The unchanged-row skip. The `github` target posts to a blind read-and-FLIP
    // endpoint, so emitting an unchanged row would invert the setting.
    if (value === originalOf(d)) continue
    // The password has its own path (`savePassword`) and never rides a batch.
    if (d.writeTarget === "password") continue
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
  return {
    identity: Object.keys(identity).length ? identity : null,
    settings: settingsBody({ ui, capabilities, defaults }),
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
          const said = wireProse(reply.message, reply.segments)
          if (reply.warning) notifyWarning(said)
          else notifyInfo(said)
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
  clearSearchRef,
}: {
  saving: boolean
  setSaving: (v: boolean) => void
  savingRef: React.RefObject<boolean>
  /** Filled in with "clear the search if there is one, saying whether there
   * was", so the dialog's Escape clears a search before it closes anything. */
  clearSearchRef: React.RefObject<(() => boolean) | null>
}) {
  const dux = useDux()
  const { bootstrap } = dux
  // `overrides` holds only the fields touched in this dialog session; every other
  // row renders live bootstrap, so another client's change is not reverted on Save.
  const [overrides, setOverrides] = useState<Record<string, SettingValue>>({})
  // The password row's fields, kept out of `overrides`: they are never a
  // config value, and they leave this component only through `savePassword`.
  const authPhase = useAuthPhase()
  const authStatus = authPhase.kind === "open" ? authPhase.status : null
  const [passwordDraft, setPasswordDraft] = useState<PasswordDraft>(EMPTY_PASSWORD_DRAFT)
  const [passwordError, setPasswordError] = useState<string | null>(null)
  const strengthReading = usePasswordStrength(passwordDraft.next)
  // Opening Preferences reads the sign-in status again, so the row shows what
  // the server says now (a password set from the CLI a moment ago included).
  useEffect(() => {
    void refreshAuthStatus()
  }, [])

  // The search. Rows it hides keep their edits in `overrides`, so Save still
  // writes them: hiding a row is a view, never an abandon.
  const [query, setQuery] = useState("")
  const words = searchWords(query)
  const searching = words.length > 0
  useEffect(() => {
    clearSearchRef.current = () => {
      if (query === "") return false
      setQuery("")
      return true
    }
    return () => {
      clearSearchRef.current = null
    }
  }, [query, clearSearchRef])
  // Type-immediately on a computer; on a phone or a touch screen focusing the
  // field would pop the soft keyboard over the dialog the moment it opens.
  const isMobile = useIsMobile()
  const coarsePointer = useIsCoarsePointer()
  const searchAutoFocus = !isMobile && !coarsePointer
  // A password error stops or ends a Save, and it is said on the password row,
  // so a search that hides that row is cleared to bring the reason on screen.
  const showPasswordError = (message: string) => {
    setPasswordError(message)
    setQuery("")
  }
  const [notificationPermission, setNotificationPermission] = useState(
    initialNotificationPermission,
  )

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

  // The password's own write. Sent last, because success signs every browser
  // out, this one included; the gate then decides what this page shows.
  const savePassword = async (write: { current?: string; next: string }): Promise<boolean> => {
    const answer = await changePassword(write)
    switch (answer.kind) {
      case "ok":
        setPasswordDraft(EMPTY_PASSWORD_DRAFT)
        setPasswordError(null)
        notifySuccess(
          write.current === undefined
            ? "Password set. Browsers that reach dux from where it applies now have to sign in with it."
            : "Password changed. Every browser was signed out and signs in again with the new one.",
        )
        return true
      case "stored_not_in_force":
        // Not a success: the file holds the new password, but problems in it
        // stop dux from using it, so the old one still applies. The drafts are
        // cleared because they are written. Sticky (weighed): the user must act
        // outside the toast, in config.toml, to put the password in force, and
        // until then the password they think they set does not apply. A warning,
        // not an error: nothing was lost and the old password still works.
        setPasswordDraft(EMPTY_PASSWORD_DRAFT)
        setPasswordError(null)
        notifyWarning(storedNotInForceSentence(answer.message), { sticky: true })
        return false
      case "refused":
        showPasswordError(answer.message)
        return false
      case "signed_out":
        reportUnauthorized()
        return false
      case "unreachable":
        showPasswordError(
          answer.timedOut
            ? "dux did not answer in time, so the password may not have changed. Check by signing in again, or try once more."
            : "Could not reach dux, so the password was not changed. Try again.",
        )
        return false
    }
  }

  const save = async () => {
    if (savingRef.current) return
    if (!requireBootstrap() || !bootstrap) return
    // Checked before anything is written: a refused password must not leave the
    // other rows half saved behind it.
    const editable =
      authStatus !== null && (authStatus.password_set || authStatus.can_set_first_password)
    const pw = editable
      ? passwordWrite(passwordDraft, {
          passwordSet: authStatus.password_set,
          mins: passwordMinimums(authStatus),
        })
      : ({ kind: "none" } as const)
    if (pw.kind === "invalid") {
      showPasswordError(pw.message)
      return
    }
    setPasswordError(null)
    savingRef.current = true
    setSaving(true)
    try {
      const touched: [SettingDescriptor, SettingValue][] = allSettingDescriptors()
        .filter((d) => d.key in overrides)
        .map((d) => [d, overrides[d.key]])
      if (!(await persist(touched, originalOf))) return
      if (pw.kind === "write") {
        const write =
          pw.current === undefined ? { next: pw.next } : { current: pw.current, next: pw.next }
        if (!(await savePassword(write))) return
        closeCustomizeWebapp()
        return
      }
      closeCustomizeWebapp()
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
        .filter((d) => lockOn(d) === null && d.writeTarget !== "password")
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

  // Whether the permission row shows under the desktop-notifications row.
  const permissionRowShown = (d: SettingDescriptor): boolean =>
    notificationPermissionShown(effective(d) as boolean, notificationPermission)

  // Every other sentence a row shows, so it is searched as well as lit.
  const extraShownText = (d: SettingDescriptor): string[] => {
    if (d.control.kind === "password") return [passwordNote(authStatus)]
    if (d.key === "capabilities.web_notifications" && permissionRowShown(d)) {
      return Object.values(NOTIFICATION_PERMISSION_TEXT)
    }
    return []
  }

  const shownGroups = SETTING_GROUPS.map((group) => ({
    group,
    settings: searching
      ? group.settings.filter((d) =>
          rowMatches(settingSearchFields(d, lockOn(d), extraShownText(d)), words),
        )
      : group.settings,
  })).filter(({ settings }) => settings.length > 0)

  return (
    // A flex column rather than the popup's grid, so the list is the one part
    // that gives up height when the popup hits its cap (a phone, a short
    // window): the header, the search and the footer stay put and only the
    // rows scroll.
    <DialogContent className="flex max-h-[85vh] flex-col sm:max-w-2xl">
      <DialogHeader className="shrink-0">
        <DialogTitle>Settings</DialogTitle>
        <DialogDescription>
          {renderInlineCode(
            "Configure dux. Saved to `config.toml` and applied to every connected browser.",
          )}
        </DialogDescription>
      </DialogHeader>

      <div className="flex shrink-0 items-center gap-2 rounded-md border border-input bg-input/30 px-3 transition-colors has-[input:focus-visible]:border-ring has-[input:focus-visible]:ring-3 has-[input:focus-visible]:ring-ring/50 max-md:min-h-10">
        <Search className="size-4 shrink-0 text-muted-foreground" />
        <input
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder="Search settings"
          aria-label="Search settings"
          autoFocus={searchAutoFocus}
          className="min-w-0 flex-1 bg-transparent py-2 text-sm outline-none placeholder:text-muted-foreground"
        />
      </div>

      {/* A fixed height rather than a cap, so the dialog does not shrink as a
          search narrows the list; it gives way only to the popup's own cap. */}
      {shownGroups.length === 0 ? (
        <div
          role="status"
          className="flex h-[60vh] min-h-0 shrink flex-col items-center justify-center gap-2 px-6 text-center"
        >
          <ZoomOut aria-hidden className="size-7 text-muted-foreground/70" />
          <p className="text-sm font-medium">
            No settings match <InlineCode>{query.trim()}</InlineCode>.
          </p>
          <p className="max-w-sm text-xs text-muted-foreground">
            This dialog holds the everyday settings. Everything else in{" "}
            <InlineCode>config.toml</InlineCode> is in the app menu, under{" "}
            <span className="font-medium text-foreground">{EDIT_CONFIG_TITLE}</span>
          </p>
        </div>
      ) : (
        <ScrollArea className="h-[60vh] min-h-0 shrink pr-3">
          <div className="flex flex-col gap-6">
            {shownGroups.map(({ group, settings }) => (
              <div key={group.surface} className="flex flex-col gap-1">
                <div className="flex items-center justify-between gap-3">
                  <p className="text-xs font-medium text-muted-foreground">
                    {group.caption}
                  </p>
                  {/* A password has no default to reset to. A search hides the
                      button: the section on screen is not the whole section. */}
                  {!searching && group.settings.some((d) => d.writeTarget !== "password") ? (
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
                  ) : null}
                </div>
                <div className="divide-y divide-border">
                  {settings.map((d) => (
                    <Fragment key={d.key}>
                      {d.control.kind === "password" ? (
                        <PasswordSettingRow
                          d={d}
                          status={authStatus}
                          draft={passwordDraft}
                          onDraft={(next) => {
                            setPasswordDraft(next)
                            setPasswordError(null)
                          }}
                          reading={strengthReading}
                          error={passwordError}
                          disabled={saving}
                          words={words}
                        />
                      ) : (
                        <SettingRow
                          d={d}
                          value={effective(d)}
                          onChange={(v) => setOverride(d.key, v)}
                          disabled={saving}
                          lock={lockOn(d)}
                          availableProviders={bootstrap?.available_providers ?? []}
                          words={words}
                        />
                      )}
                      {/* Directly beneath the setting it is a precondition for. */}
                      {d.key === "capabilities.web_notifications" && permissionRowShown(d) ? (
                        <NotificationPermissionRow
                          setPermission={setNotificationPermission}
                          words={words}
                        />
                      ) : null}
                    </Fragment>
                  ))}
                </div>
              </div>
            ))}
          </div>
        </ScrollArea>
      )}

      {/* Misclick-safe spacing between the last row and the footer buttons. */}
      <div className="h-2 shrink-0" />
      <DialogFooter className="shrink-0">
        <div className="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
          {/* Cancel is disabled too while a write is in flight: closing the
              dialog mid-persist would let the pending success close a freshly
              reopened dialog session, or misread as "the edit was discarded"
              when the request still lands on the server. */}
          <Button
            variant="outline"
            autoFocus={!searchAutoFocus}
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
  const clearSearchRef = useRef<(() => boolean) | null>(null)

  return (
    <Dialog
      open={customizeWebappOpen}
      onOpenChange={(o, details) => {
        // Escape over a search clears the search and keeps the dialog open;
        // the next Escape closes it. Back and every other dismissal close as
        // they always have.
        if (!o && details.reason === "escape-key" && clearSearchRef.current?.()) {
          details.cancel()
          return
        }
        if (!o && !savingRef.current) closeCustomizeWebapp()
      }}
    >
      {customizeWebappOpen && (
        <CustomizeWebappForm
          saving={saving}
          setSaving={setSaving}
          savingRef={savingRef}
          clearSearchRef={clearSearchRef}
        />
      )}
    </Dialog>
  )
}
