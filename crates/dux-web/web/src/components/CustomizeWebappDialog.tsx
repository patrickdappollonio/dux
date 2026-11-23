import { Fragment, useEffect, useRef, useState } from "react"
import type { KeyboardEvent, RefObject } from "react"
import {
  Check,
  ChevronRight,
  CircleCheck,
  CircleDot,
  FileCode,
  RotateCcw,
  Search,
  Undo2,
  ZoomOut,
  type LucideIcon,
} from "lucide-react"
import {
  notifyError,
  notifyInfo,
  notifySuccess,
  notifyWarning,
} from "@/lib/notify"

import { unsavedChangesLabel } from "@/lib/unsavedChanges"
import { ExplainedWhenDisabled } from "@/components/ExplainedWhenDisabled"
import { Modal, useModalLifetime, type ModalCloseReason } from "@/components/modal"
import {
  CHANGED_LABEL,
  TOPIC_ICON,
  buildWrites,
  defaultValueText,
  listNames,
  resetValue,
  rowMatches,
  rowMeta,
  sameValue,
  searchWords,
  settingSearchFields,
  settingsLabel,
  topicLabel,
  valueText,
  zeroText,
  type View,
} from "@/components/preferencesModel"
import { MatchHighlight } from "@/components/MatchHighlight"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import { EmptyState } from "@/components/ui/empty"
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
import { FOCUS_RING } from "@/lib/focusRing"
import {
  SETTING_TOPICS,
  allSettingDescriptors,
  topicSettings,
  type SettingDescriptor,
  type SettingTopic,
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
import { renderInlineCode } from "@/lib/inlineMarkdown"
import { matchCharRanges } from "@/lib/searchMatch"
import { useIsCoarsePointer } from "@/hooks/use-coarse-pointer"
import { useIsMobile } from "@/hooks/use-mobile"
import { chip, wireProse } from "@/lib/prose"
import {
  changesPaneVisible,
  closeCustomizeWebapp,
  currentBootstrap,
  mobileAccessoryBarVisible,
  openConfigEditor,
  saveSettings,
  setAsidePreferencesDraft,
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

// A config key wraps after a dot or an underscore, never inside a word.
function ConfigKey({ text, words }: { text: string; words: string[] }) {
  const ranges = matchCharRanges(text, words)
  const pieces = text.match(/[^._]+[._]?|[._]/g) ?? [""]
  let offset = 0
  return pieces.map((piece, i) => {
    const at = offset
    offset += Array.from(piece).length
    const own = ranges
      .map((r) => ({ start: r.start - at, end: r.end - at }))
      .filter((r) => r.end > 0)
      .map((r) => ({ start: Math.max(0, r.start), end: r.end }))
    return (
      <Fragment key={at}>
        {i > 0 ? <wbr /> : null}
        <MatchHighlight text={piece} ranges={own} />
      </Fragment>
    )
  })
}

// Under a row's prose: its config key while a search runs, so a key typed from
// `config.toml` shows why the row matched, then where the setting applies.
// Only what the search reads is lit.
function RowFooter({
  d,
  words,
  meta,
}: {
  d: SettingDescriptor
  words: string[]
  meta: ReturnType<typeof rowMeta>
}) {
  return (
    <>
      {words.length > 0 ? (
        <span className="font-mono text-xs break-words text-muted-foreground">
          <ConfigKey text={d.key} words={words} />
        </span>
      ) : null}
      <p className="text-xs text-muted-foreground">
        {meta.map((part, i) => (
          <Fragment key={part.text}>
            {i > 0 ? " · " : null}
            <MatchHighlight
              text={part.text}
              ranges={part.searched ? matchCharRanges(part.text, words) : []}
            />
          </Fragment>
        ))}
      </p>
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

// An out-of-range configured value degrades to the documented default instead
// (`clampTerminalFontSize`); a number typed here is brought to the nearer bound.
function clampToControl(n: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, n))
}

// A number typed out of range is brought into range on leaving the field or on
// Save, never per keystroke, so typing 16 into a field whose minimum is 8 can
// pass through 1. Local text keeps an empty field without committing it as zero.
function NumberControl({
  id,
  label,
  min,
  max,
  unit,
  value,
  onChange,
  disabled,
}: {
  id: string
  label: string
  min: number
  max: number
  unit?: string
  value: number
  /** `typed` is the out-of-range number a clamp on leaving the field replaced. */
  onChange: (v: number, typed?: number) => void
  disabled: boolean
}) {
  const [text, setText] = useState(String(value))
  const [prevValue, setPrevValue] = useState(value)
  if (value !== prevValue) {
    setPrevValue(value)
    setText(String(value))
  }

  return (
    <div className="flex items-center gap-2">
    <Input
      id={id}
      type="number"
      aria-label={label}
      min={min}
      max={max}
      value={text}
      disabled={disabled}
      className="w-24"
      onChange={(e) => {
        const raw = e.target.value
        setText(raw)
        if (raw.trim() === "") return
        const parsed = Number(raw)
        if (!Number.isFinite(parsed)) return
        onChange(parsed)
      }}
      onBlur={() => {
        // Leaving the field empty reverts the displayed text to the last committed
        // value, rather than stranding a blank input over a stale override.
        if (text.trim() === "") {
          setText(String(value))
          return
        }
        const inRange = clampToControl(value, min, max)
        if (inRange !== value) onChange(inRange, value)
      }}
    />
    {unit ? <span className="text-xs text-muted-foreground">{unit}</span> : null}
    </div>
  )
}

// A number still out of range when Save is pressed (its field never lost focus)
// is saved at the nearer bound, the value leaving the field would have given.
function inControlRange(d: SettingDescriptor, v: SettingValue): SettingValue {
  return d.control.kind === "number" && typeof v === "number"
    ? clampToControl(v, d.control.min, d.control.max)
    : v
}

/// What Save says of a number it brought into range: saved in place of the one
/// typed, or kept, when the value in range is the one already saved.
function broughtIntoRangeSentence(
  d: SettingDescriptor,
  value: SettingValue,
  typed: SettingValue,
  saved: boolean,
): string {
  const unit = d.control.kind === "number" && d.control.unit ? ` ${d.control.unit}` : ""
  const range =
    d.control.kind === "number" ? `: it goes from ${d.control.min} to ${d.control.max}` : ""
  const verb = saved ? "was saved as" : "stays"
  return `${d.label} ${verb} ${String(value)}${unit}, not ${String(typed)}${range}.`
}

/// What one Save press says of the rows it saved through the settings write,
/// which the server keeps quiet on the web, of every number it brought into
/// range, and of the rows the server already held, or null when it has nothing
/// to say. The other writes answer
/// their own outcome on the wire.
function saveConfirmation(
  saved: readonly SettingDescriptor[],
  broughtIn: readonly { d: SettingDescriptor; value: SettingValue; typed: SettingValue }[],
  already: readonly SettingDescriptor[],
): string | null {
  const quiet = saved.filter((d) => d.writeTarget === "settings")
  const plain = quiet.filter((d) => !broughtIn.some((b) => b.d.key === d.key))
  const parts: string[] = []
  if (plain.length > 0) parts.push(`Saved ${listNames(plain.map((d) => d.label))}.`)
  for (const { d, value, typed } of broughtIn) {
    parts.push(broughtIntoRangeSentence(d, value, typed, saved.includes(d)))
  }
  if (already.length > 0) {
    parts.push(
      already.length === 1
        ? `${already[0].label} was already set that way, so it was not saved again.`
        : `${listNames(already.map((d) => d.label))} were already set that way, so they were not saved again.`,
    )
  }
  if (quiet.some((d) => d.surface !== "tui")) {
    parts.push("Every connected browser picks up the change now.")
  }
  if (quiet.some((d) => d.surface === "tui" || d.surface === "both")) {
    parts.push("A running terminal app applies it after its next config reload or restart.")
  }
  return parts.length > 0 ? parts.join(" ") : null
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
  onChange: (v: SettingValue, typed?: number) => void
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
          onCheckedChange={(v) => onChange(v)}
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
          unit={d.control.unit}
          value={value as number}
          disabled={disabled}
          onChange={onChange}
        />
      )
    case "enum":
      return (
        <Select
          value={value as string}
          onValueChange={(v) => onChange(v as string)}
          disabled={disabled}
        >
          <SelectTrigger id={id} aria-label={d.label}>
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
          <SelectTrigger id={id} aria-label={d.label}>
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
          className="w-full md:w-56"
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
    <div className="flex flex-col gap-2 border-b py-3 md:flex-row md:items-start md:justify-between md:gap-6">
      <div className="flex flex-col gap-1">
        <span className="text-sm font-medium">{lit(NOTIFICATION_PERMISSION_TEXT.title)}</span>
        <p className="text-xs text-muted-foreground">{lit(NOTIFICATION_PERMISSION_TEXT.body)}</p>
      </div>
      <div className="shrink-0 md:pt-0.5">
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="max-md:min-h-10 pointer-coarse:min-h-10"
          onClick={() => void request()}
        >
          {lit(NOTIFICATION_PERMISSION_TEXT.action)}
        </Button>
      </div>
    </div>
  )
}


// A row's reset sits beside its control at the control's height: 32px for a
// mouse, 40px under a finger.
const RESET_SIZE = "size-8 max-md:size-10 pointer-coarse:size-10"

function SettingRow({
  d,
  value,
  onChange,
  onReset,
  onUndo,
  changed,
  saved,
  savedMoved,
  meta,
  disabled,
  lock,
  availableProviders,
  words,
  phone,
  rowRef,
}: {
  d: SettingDescriptor
  value: SettingValue
  onChange: (v: SettingValue, typed?: number) => void
  onReset: () => void
  onUndo: () => void
  /** The draft differs from the documented default: a dot and a reset. */
  changed: boolean
  /** The saved value while the draft differs from it, else null. */
  saved: SettingValue | null
  /** The saved value is no longer the one the edit was made over. */
  savedMoved: boolean
  /** The small line: where the setting applies, and more (see `rowMeta`). */
  meta: ReturnType<typeof rowMeta>
  disabled: boolean
  /** The sentence saying why this run cannot change the row, or null. */
  lock: string | null
  availableProviders: string[]
  /** The search's words, lit wherever they occur; empty when not searching. */
  words: string[]
  phone: boolean
  rowRef: (el: HTMLDivElement | null) => void
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
  const zero = zeroText(d)
  // On a phone a field or the swatches take the row's whole width below the
  // prose; every other control stays beside it.
  const stacked = phone && (d.control.kind === "text" || d.control.kind === "favicon")
  return (
    <div
      ref={rowRef}
      data-setting-row={d.key}
      className={cn("flex gap-3 border-b py-3", stacked ? "flex-col" : "items-start")}
    >
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <div className="flex items-center gap-1.5">
          {changed ? (
            <>
              <span aria-hidden className="size-1.5 shrink-0 rounded-full bg-primary" />
              <span className="sr-only">Changed from default:</span>
            </>
          ) : null}
          {labelEl}
        </div>
        <p className="text-xs text-muted-foreground">
          {renderInlineCode(lock ?? d.description, words)}
        </p>
        {zero ? (
          <p className="text-xs text-muted-foreground">
            <MatchHighlight text={zero} ranges={matchCharRanges(zero, words)} />
          </p>
        ) : null}
        {saved !== null ? (
          <p className={cn("text-xs", savedMoved ? "text-dux-warning" : "text-muted-foreground")}>
            {savedMoved
              ? `Not saved yet. The saved value changed to ${valueText(d, saved)} since this edit was made.`
              : `Not saved yet. Saved: ${valueText(d, saved)}`}
          </p>
        ) : null}
        <RowFooter d={d} words={words} meta={meta} />
      </div>
      <div
        data-setting-control
        className={cn("flex items-center gap-1", stacked ? "w-full" : "shrink-0")}
      >
        {changed ? (
          <SimpleTooltip content="Reset to default">
            <Button
              type="button"
              variant="ghost"
              size="icon"
              data-reset
              aria-label={`Reset ${d.label} to ${defaultValueText(d)}`}
              disabled={disabled}
              className={cn("shrink-0", RESET_SIZE)}
              onClick={onReset}
            >
              <RotateCcw />
            </Button>
          </SimpleTooltip>
        ) : null}
        {saved !== null ? (
          <SimpleTooltip content="Put back the saved value">
            <Button
              type="button"
              variant="ghost"
              size="icon"
              data-undo
              aria-label={`Put ${d.label} back to its saved value, ${valueText(d, saved)}`}
              disabled={disabled}
              className={cn("shrink-0", RESET_SIZE)}
              onClick={onUndo}
            >
              <Undo2 />
            </Button>
          </SimpleTooltip>
        ) : null}
        <div className={cn(stacked && "min-w-0 flex-1")}>
          <SettingControl
            d={d}
            value={value}
            onChange={onChange}
            disabled={disabled}
            availableProviders={availableProviders}
          />
        </div>
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

// Stacked a gap-3 apart, at the shared input's heights.
const PASSWORD_FIELD_CLASS = "w-full md:w-64"

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

// The one row that is not a `SettingRow`, sent through its own write target by
// Save. Which fields show is the server's answer, read again on every opening.
function PasswordSettingRow({
  d,
  status,
  draft,
  onDraft,
  reading,
  error,
  disabled,
  words,
  meta,
}: {
  d: SettingDescriptor
  status: AuthStatus | null
  draft: PasswordDraft
  onDraft: (next: PasswordDraft) => void
  reading: StrengthReading
  error: string | null
  disabled: boolean
  words: string[]
  /** The row's small line: where it applies, and its topic where shown. */
  meta: ReturnType<typeof rowMeta>
}) {
  const id = rowId(d)
  const editable =
    status !== null && (status.password_set || status.can_set_first_password)
  const mins = status ? passwordMinimums(status) : null
  return (
    <div data-setting-row={d.key} className="flex flex-col gap-3 border-b py-3">
      <div className="flex flex-col gap-1">
        <span id={`${id}-label`} className="text-sm font-medium">
          <MatchHighlight text={d.label} ranges={matchCharRanges(d.label, words)} />
        </span>
        <p className="text-xs text-muted-foreground">
          {renderInlineCode(d.description, words)}
        </p>
        <p className="text-xs text-muted-foreground">
          {renderInlineCode(passwordNote(status), words)}
        </p>
        <RowFooter d={d} words={words} meta={meta} />
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

// The keys of the rows sent, by whether their write landed (each refusal raised
// its own error). Both empty: nothing differed, so nothing was sent.
interface PersistResult {
  saved: string[]
  failed: string[]
}

// The identity, changes-pane and GitHub writes answer their own outcome with an
// engine status and Tailscale with a reply sentence; the settings PATCH's status
// is quiet on the web, so Save confirms those rows itself (`saveConfirmation`).
async function persist(
  entries: [SettingDescriptor, SettingValue][],
  originalOf: (d: SettingDescriptor) => SettingValue,
): Promise<PersistResult> {
  const { identity, settings, changesPane, github, tailscale } = buildWrites(
    entries,
    originalOf,
  )
  const writes = new Map<SettingDescriptor["writeTarget"], Promise<boolean>>()
  if (identity) writes.set("identity", setInstanceIdentity(identity))
  if (settings) writes.set("settings", saveSettings(settings))
  if (changesPane !== null) writes.set("changesPane", setChangesPaneVisibility(changesPane))
  // `github` is non-null ONLY when the row changed, which is what makes it safe
  // to drive a read-and-flip endpoint from an explicit-value UI.
  if (github !== null) {
    writes.set(
      "github",
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
    writes.set(
      "tailscale",
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
  const targets = [...writes.keys()]
  const outcomes = await Promise.all(writes.values())
  const landed = new Set(targets.filter((_, i) => outcomes[i]))
  const result: PersistResult = { saved: [], failed: [] }
  for (const [d, value] of entries) {
    if (d.writeTarget === "password" || sameValue(d, value, originalOf(d))) continue
    result[landed.has(d.writeTarget) ? "saved" : "failed"].push(d.key)
  }
  return result
}


// The one height of the controls that sit together in the side list (the
// search, the topics, Open config.toml…): 32px for a mouse, 40px under a finger.
const CONTROL_HEIGHT = "h-8 max-md:h-10 pointer-coarse:h-10"

// The side list: "Changed from default", then the topics. Each is a button, in
// the tab order, and the arrow keys, Home and End move between them.
function TopicNav({
  view,
  changedCount,
  topicHasChanges,
  dim,
  phone,
  navEls,
  onPick,
}: {
  /** The view on screen, or null while a search replaces it. */
  view: View | null
  changedCount: number
  topicHasChanges: (topic: SettingTopic) => boolean
  dim: boolean
  phone: boolean
  navEls: RefObject<Map<View, HTMLButtonElement>>
  onPick: (view: View) => void
}) {
  function handleKeyDown(event: KeyboardEvent<HTMLElement>) {
    const items = [...event.currentTarget.querySelectorAll<HTMLButtonElement>("[data-topic]")]
    const at = items.findIndex((item) => item === document.activeElement)
    const next =
      event.key === "ArrowDown"
        ? items[Math.min(at + 1, items.length - 1)]
        : event.key === "ArrowUp"
          ? items[Math.max(at - 1, 0)]
          : event.key === "Home"
            ? items[0]
            : event.key === "End"
              ? items[items.length - 1]
              : undefined
    if (next === undefined) return
    event.preventDefault()
    next.focus()
  }

  const item = (id: View, label: string, Icon: LucideIcon, count: number, dot: boolean) => (
    <button
      key={id}
      type="button"
      data-topic={id}
      ref={(el) => {
        if (el) navEls.current.set(id, el)
        else navEls.current.delete(id)
      }}
      aria-current={view === id ? "page" : undefined}
      onClick={() => onPick(id)}
      className={cn(
        "flex w-full shrink-0 items-center gap-2 rounded-md border border-transparent px-2 text-sm text-muted-foreground transition-colors hover:bg-muted/60 hover:text-foreground",
        FOCUS_RING,
        phone ? "h-12" : "h-8 pointer-coarse:h-10",
        view === id && "bg-muted font-medium text-foreground hover:bg-muted",
      )}
    >
      <Icon aria-hidden className="size-4 shrink-0" />
      <span className="min-w-0 flex-1 truncate text-left">{label}</span>
      {dot ? (
        <>
          <span aria-hidden className="size-1.5 shrink-0 rounded-full bg-primary" />
          <span className="sr-only">, some changed from default</span>
        </>
      ) : null}
      <span aria-hidden className="text-xs text-muted-foreground tabular-nums">
        {count}
      </span>
      <span className="sr-only">, {settingsLabel(count)}</span>
      {phone ? <ChevronRight aria-hidden className="size-4 shrink-0 text-muted-foreground" /> : null}
    </button>
  )

  return (
    <nav
      aria-label="Preferences topics"
      onKeyDown={handleKeyDown}
      className={cn("flex flex-col gap-0.5", dim && "opacity-50")}
    >
      {item("changed", CHANGED_LABEL, CircleDot, changedCount, false)}
      <div aria-hidden className="my-1.5 h-px shrink-0 bg-border" />
      {SETTING_TOPICS.map((t) =>
        item(t.id, t.label, TOPIC_ICON[t.id], topicSettings(t.id).length, topicHasChanges(t.id)),
      )}
    </nav>
  )
}

function PreferencesForm({ open, onClosed }: { open: boolean; onClosed: () => void }) {
  const phone = useIsMobile()
  // One persist at a time: the ref gates re-entry synchronously, and the state
  // makes the modal busy, so no way out orphans a write.
  const savingRef = useRef(false)
  const [saving, setSaving] = useState(false)
  const dux = useDux()
  const { bootstrap } = dux
  // Read once: what this opening hands the form.
  const [start] = useState(() => ({
    topic: dux.customizeWebappTopic ?? null,
    draft: dux.customizeWebappDraft ?? { values: {}, saved: {} },
  }))
  // `overrides` holds only the fields touched in this dialog session; every other
  // row renders live bootstrap, so another client's change is not reverted on Save.
  // Every edit and every reset lands here and nothing else: Save writes them.
  const [overrides, setOverrides] = useState<Record<string, SettingValue>>(start.draft.values)
  // The saved value each override was made over, so a row can say when the
  // saved value has moved since.
  const [editedOver, setEditedOver] = useState<Record<string, SettingValue>>(start.draft.saved)
  // Rows a write from this dialog has saved, with the bootstrap they were
  // written over. The written value is the saved one until a bootstrap arrives.
  const [written, setWritten] = useState<
    Record<string, { value: SettingValue; over: typeof bootstrap }>
  >({})
  // What the last Save that only partly went through saved and did not.
  const [partialSave, setPartialSave] = useState<{ saved: string[]; failed: string[] } | null>(
    null,
  )
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

  // The search covers every topic at once. A row it hides keeps its edit, and
  // Save still writes it: hiding a row is a view, never an abandon.
  const [query, setQuery] = useState("")
  const words = searchWords(query)
  const searching = words.length > 0
  const [view, setView] = useState<View>(start.topic ?? "look")
  // On a phone the topics come first and a topic's rows are the cover's second
  // step: its title row carries the way back, and the browser's Back takes it.
  const [step, setStep] = useState<"topics" | "topic">(start.topic ? "topic" : "topics")
  // The rows a confirmed "Reset all" puts back, frozen while its confirm is up.
  const [resetAll, setResetAll] = useState<{ keys: string[]; names: string[] } | null>(null)
  const [notificationPermission, setNotificationPermission] = useState(
    initialNotificationPermission,
  )

  const coarsePointer = useIsCoarsePointer()
  const searchRef = useRef<HTMLInputElement | null>(null)
  // The title row's Back, which the modal draws on the phone's second step.
  const backRef = useRef<HTMLButtonElement | null>(null)
  const navEls = useRef(new Map<View, HTMLButtonElement>())
  const rowEls = useRef(new Map<string, HTMLDivElement>())
  // The row that takes focus once a reset or an undo has removed the button
  // pressed, and the button it prefers, else the row's control.
  const focusAfter = useRef<{ key: string; prefer: string } | null>(null)
  // Whether the phone's topics are on screen again after a topic was left.
  const leftTopic = useRef(false)
  // Set by a confirmed "Reset all", for the confirm's close to read.
  const resetAllDone = useRef(false)

  // A password error stops or ends a Save, and it is said on the password row,
  // so the row is brought on screen: the search cleared and its topic shown.
  const showPasswordError = (message: string) => {
    setPasswordError(message)
    setQuery("")
    setView("network")
    if (phone) setStep("topic")
  }

  // The pre-touch baseline for a row. Any field the store tracks an optimistic
  // override for must read the override-aware selector rather than raw bootstrap:
  // the rule is override-awareness, not `writeTarget`.
  const originalIn = (d: SettingDescriptor, b: typeof bootstrap): SettingValue => {
    const saved = written[d.key]
    if (saved !== undefined && saved.over === b) return saved.value
    if (d.writeTarget === "changesPane") return changesPaneVisible(dux)
    if (d.key === "ui.mobile_accessory_bar") return mobileAccessoryBarVisible(dux)
    return b ? d.read(b) : d.default
  }
  const originalOf = (d: SettingDescriptor): SettingValue => originalIn(d, bootstrap)

  const effective = (d: SettingDescriptor): SettingValue => {
    if (d.key in overrides) return overrides[d.key]
    return originalOf(d)
  }

  // A row edited, reset or put back stays in the Changed view or the search
  // results it was in until that list is left.
  const listScope = searching ? `search ${query}` : view
  const [kept, setKept] = useState<{ scope: string; keys: string[] }>({ scope: "", keys: [] })
  const keptHere = kept.scope === listScope ? kept.keys : []
  const keep = (keys: string[]) =>
    setKept((k) => ({
      scope: listScope,
      keys: [...(k.scope === listScope ? k.keys : []), ...keys],
    }))

  // The number a field held before leaving it brought it into range, by key,
  // for Save to say what it saved in its place.
  const typedOutOfRange = useRef<Record<string, number>>({})
  const setOverride = (d: SettingDescriptor, value: SettingValue, typed?: number) => {
    if (typed === undefined) delete typedOutOfRange.current[d.key]
    else typedOutOfRange.current[d.key] = typed
    setOverrides((o) => ({ ...o, [d.key]: value }))
    setEditedOver((e) => ({ ...e, [d.key]: originalOf(d) }))
    keep([d.key])
  }
  const dropOverrides = (keys: string[]) => {
    const drop = (o: Record<string, SettingValue>) =>
      Object.fromEntries(Object.entries(o).filter(([k]) => !keys.includes(k)))
    setOverrides(drop)
    setEditedOver(drop)
  }

  // What this RUN of the server cannot put in force for the row, if anything.
  // The row stays editable: the value is saved for the next run.
  const lockOn = (d: SettingDescriptor): string | null =>
    bootstrap ? (d.lockedBy?.(bootstrap) ?? null) : null

  const descriptors = allSettingDescriptors()
  // Neither the password nor a row this server does not publish (until edited
  // here) has a value anyone chose, so neither is changed nor reset.
  const resettable = (d: SettingDescriptor) =>
    d.writeTarget !== "password" &&
    (d.key in overrides || bootstrap === null || d.published?.(bootstrap) !== false)
  const differs = (d: SettingDescriptor) =>
    resettable(d) && !sameValue(d, effective(d), d.default)
  const changedSettings = descriptors.filter(differs)
  // A row reset here stays in the Changed list until it is saved, so the
  // reset can be seen and undone where it was made.
  const changedList = descriptors.filter(
    (d) =>
      differs(d) ||
      (resettable(d) && !sameValue(d, originalOf(d), d.default)) ||
      keptHere.includes(d.key),
  )
  const unsaved = (d: SettingDescriptor) =>
    d.writeTarget !== "password" &&
    d.key in overrides &&
    !sameValue(d, overrides[d.key], originalOf(d))
  const unsavedSettings = descriptors.filter(unsaved)
  const passwordTyped =
    passwordDraft.current !== "" || passwordDraft.next !== "" || passwordDraft.confirm !== ""
  const unsavedCount = unsavedSettings.length + (passwordTyped ? 1 : 0)

  // After a reset or an undo its button is gone, so focus moves to the button
  // that took its place, else the control beside it.
  useEffect(() => {
    const after = focusAfter.current
    if (after === null) return
    focusAfter.current = null
    const row = rowEls.current.get(after.key)
    ;(
      row?.querySelector<HTMLElement>(after.prefer) ??
      row?.querySelector<HTMLElement>(
        '[data-setting-control] :is(button, input, [role="switch"], [role="combobox"])',
      )
    )?.focus()
  })

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
    setPartialSave(null)
    savingRef.current = true
    setSaving(true)
    try {
      const touched: [SettingDescriptor, SettingValue][] = descriptors
        .filter((d) => d.key in overrides)
        .map((d) => [d, inControlRange(d, overrides[d.key])])
      // A number brought into range, here or on leaving its field, is shown in
      // its field at once and said once it is saved, so nobody is left
      // believing the typed one was.
      const broughtIn = touched.flatMap(([d, value]) => {
        const typed =
          value !== overrides[d.key] ? overrides[d.key] : typedOutOfRange.current[d.key]
        return typed === undefined ? [] : [{ d, value, typed }]
      })
      // The typed number is kept with the value in range, so a save that
      // fails and is pressed again still says what it brought into range.
      for (const { d, value, typed } of broughtIn) {
        setOverride(d, value, typeof typed === "number" ? typed : undefined)
      }
      // Compared against what the server holds now rather than what this page
      // last heard: the config can change under an open dialog, and a row it
      // already holds is neither sent nor said to be saved.
      const over = (await currentBootstrap()) ?? bootstrap
      const savedValue = (d: SettingDescriptor) => originalIn(d, over)
      const already = touched
        .filter(
          ([d, value]) =>
            d.writeTarget !== "password" &&
            !sameValue(d, value, originalOf(d)) &&
            sameValue(d, value, savedValue(d)),
        )
        .map(([d]) => d)
      if (already.length > 0) dropOverrides(already.map((d) => d.key))
      const persisted = await persist(touched, savedValue)
      // A landed write leaves the draft, so the count and a retry hold only
      // what is still unsaved, and a read-and-flip row is never sent twice.
      if (persisted.saved.length > 0) {
        setWritten((w) => ({
          ...w,
          ...Object.fromEntries(
            touched
              .filter(([d]) => persisted.saved.includes(d.key))
              .map(([d, value]) => [d.key, { value, over }]),
          ),
        }))
        dropOverrides(persisted.saved)
      }
      // One confirmation for the whole press, said for the part that was saved
      // even when another part failed: a saved row leaves the draft, so this is
      // the only place its number brought into range is ever said. Every other
      // row keeps its note in the draft for the retry to say.
      const confirmation = saveConfirmation(
        descriptors.filter((d) => persisted.saved.includes(d.key)),
        persisted.failed.length === 0
          ? broughtIn
          : broughtIn.filter(({ d }) => persisted.saved.includes(d.key)),
        already,
      )
      if (confirmation !== null) {
        if (persisted.saved.length > 0) notifySuccess(confirmation)
        else notifyInfo(confirmation)
      }
      if (persisted.failed.length > 0) {
        if (persisted.saved.length > 0) setPartialSave(persisted)
        return
      }
      if (pw.kind === "write") {
        const write =
          pw.current === undefined ? { next: pw.next } : { current: pw.current, next: pw.next }
        if (!(await savePassword(write))) return
        closeCustomizeWebapp()
        return
      }
      // The action did nothing because every row already held: said, because a
      // dialog closing in silence reads the same as a save that went nowhere.
      if (persisted.saved.length === 0 && broughtIn.length === 0 && already.length === 0) {
        notifyInfo("Nothing to save: no preference was changed.")
      }
      closeCustomizeWebapp()
    } finally {
      savingRef.current = false
      setSaving(false)
    }
  }

  // A reset changes the draft only. A locked row is reset too: its lock says
  // this run cannot put the value in force, not that it cannot be saved.
  const resetRow = (d: SettingDescriptor) => {
    setOverride(d, resetValue(d))
    focusAfter.current = { key: d.key, prefer: "[data-undo]" }
  }

  // Puts the saved value back: the row's edit or reset is dropped.
  const undoRow = (d: SettingDescriptor) => {
    dropOverrides([d.key])
    keep([d.key])
    focusAfter.current = { key: d.key, prefer: "[data-reset]" }
  }

  const confirmResetAll = () => {
    for (const d of descriptors) {
      if (resetAll?.keys.includes(d.key)) setOverride(d, resetValue(d))
    }
    resetAllDone.current = true
    setResetAll(null)
  }

  const pickView = (next: View) => {
    setQuery("")
    setView(next)
    if (phone) setStep("topic")
  }

  // The config editor reads the file, which does not hold unsaved edits yet,
  // so it is offered only when there are none.
  const openConfig = () => {
    closeCustomizeWebapp()
    openConfigEditor()
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

  const found = searching
    ? descriptors.filter((d) =>
        keptHere.includes(d.key) ||
        rowMatches(settingSearchFields(d, lockOn(d), extraShownText(d)), words),
      )
    : []

  const renderRows = (list: SettingDescriptor[], showTopic: boolean) =>
    list.map((d) => (
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
            meta={rowMeta(d, false, showTopic)}
          />
        ) : (
          <SettingRow
            d={d}
            value={effective(d)}
            onChange={(v, typed) => setOverride(d, v, typed)}
            onReset={() => resetRow(d)}
            onUndo={() => undoRow(d)}
            changed={differs(d)}
            saved={unsaved(d) ? originalOf(d) : null}
            savedMoved={
              unsaved(d) && d.key in editedOver && !sameValue(d, editedOver[d.key], originalOf(d))
            }
            meta={rowMeta(d, differs(d), showTopic)}
            disabled={saving}
            lock={lockOn(d)}
            availableProviders={bootstrap?.available_providers ?? []}
            words={words}
            phone={phone}
            rowRef={(el) => {
              if (el) rowEls.current.set(d.key, el)
              else rowEls.current.delete(d.key)
            }}
          />
        )}
        {/* Directly beneath the setting it is a precondition for. */}
        {d.key === "capabilities.web_notifications" && permissionRowShown(d) ? (
          <NotificationPermissionRow setPermission={setNotificationPermission} words={words} />
        ) : null}
      </Fragment>
    ))

  const headingClass = "text-base font-medium"

  const searchResults =
    found.length === 0 ? (
      <EmptyState
        size="page"
        announce
        icon={<ZoomOut />}
        title={
          <>
            No settings match <InlineCode>{query.trim()}</InlineCode>.
          </>
        }
        description={
          <>
            This dialog holds the everyday settings. Everything else in{" "}
            <InlineCode>config.toml</InlineCode> is in the app menu, under{" "}
            <span className="font-medium text-foreground">{EDIT_CONFIG_TITLE}</span>
          </>
        }
      />
    ) : (
      <>
        <p className={phone ? "pt-1 text-xs text-muted-foreground" : headingClass}>
          {settingsLabel(found.length)} {found.length === 1 ? "matches" : "match"}{" "}
          <InlineCode>{query.trim()}</InlineCode>
        </p>
        {renderRows(found, true)}
      </>
    )

  const resetAllButton =
    changedSettings.length > 0 ? (
      <Button
        variant="outline"
        disabled={saving}
        className="shrink-0 max-md:min-h-10 pointer-coarse:min-h-10"
        onClick={() =>
          setResetAll({
            keys: changedSettings.map((d) => d.key),
            names: changedSettings.map((d) => d.label),
          })
        }
      >
        <RotateCcw />
        Reset all {changedSettings.length}…
      </Button>
    ) : null

  const changedEmpty = (
    <EmptyState
      size="page"
      icon={<CircleCheck />}
      title="Everything is at its default"
      description="Nothing here differs from what dux ships with. Settings you change show up in this list, each with its own reset."
    />
  )

  // On a phone the view's name is the cover's title, so its body opens on
  // the rows.
  const heading = (text: string) => (phone ? null : <h3 className={headingClass}>{text}</h3>)

  const viewBody =
    view === "changed" ? (
      changedList.length === 0 ? (
        changedEmpty
      ) : (
        <>
          <div className="flex items-start justify-end gap-2 pb-1">
            {phone ? null : (
              <div className="flex min-w-0 flex-1 flex-col gap-1">
                {heading(CHANGED_LABEL)}
                <p className="text-xs text-muted-foreground">
                  Everything here differs from what dux ships with. The reset on a
                  row puts that one back, and a row you reset stays until you leave.
                </p>
              </div>
            )}
            {resetAllButton}
          </div>
          {renderRows(changedList, true)}
        </>
      )
    ) : (
      <>
        {phone ? null : <div className="pb-1">{heading(topicLabel(view))}</div>}
        {renderRows(topicSettings(view), false)}
      </>
    )

  const searchBox = (
    <div
      className={cn(
        "flex shrink-0 items-center gap-2 rounded-md border border-input bg-input/30 px-3 transition-colors has-[input:focus-visible]:border-ring has-[input:focus-visible]:ring-3 has-[input:focus-visible]:ring-ring/50",
        CONTROL_HEIGHT,
      )}
    >
      <Search aria-hidden className="size-4 shrink-0 text-muted-foreground" />
      <input
        ref={searchRef}
        value={query}
        onChange={(event) => setQuery(event.target.value)}
        placeholder="Search settings"
        aria-label="Search settings"
        className="min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground"
      />
    </div>
  )

  const nav = (
    <TopicNav
      view={searching ? null : phone ? null : view}
      changedCount={changedSettings.length}
      topicHasChanges={(topic) => topicSettings(topic).some(differs)}
      dim={searching && !phone}
      phone={phone}
      navEls={navEls}
      onPick={pickView}
    />
  )

  const configHint =
    unsavedCount > 0
      ? "Save or cancel your unsaved changes first: config.toml does not have them yet."
      : null
  const openConfigButton = (
    <ExplainedWhenDisabled disabled={unsavedCount > 0} hint={configHint} className="flex w-full rounded-lg">
      <Button
        variant="ghost"
        disabled={unsavedCount > 0}
        className={cn("w-full justify-start px-2", phone ? "h-12" : CONTROL_HEIGHT)}
        onClick={openConfig}
      >
        <FileCode />
        Open config.toml…
      </Button>
    </ExplainedWhenDisabled>
  )

  const labelsOf = (keys: string[]) =>
    descriptors.filter((d) => keys.includes(d.key)).map((d) => d.label)
  const partialNotice =
    partialSave === null ? null : (
      <p role="status" className="text-xs text-dux-warning">
        {`Part of the save went through. Saved: ${listNames(labelsOf(partialSave.saved))}. Not saved: ${listNames(labelsOf(partialSave.failed))}, still marked unsaved; Save tries ${partialSave.failed.length === 1 ? "it" : "them"} again.`}
      </p>
    )

  const inTopic = phone && step === "topic"

  // What takes focus when the modal opens or changes step. With a mouse the
  // search, to type into at once; under a finger a topic, since a focused
  // field would raise the soft keyboard over the dialog. On a phone's second
  // step the way back, and back on the topics the topic that was left.
  const initialFocus = (): HTMLElement | null => {
    if (inTopic) return backRef.current
    if (!phone && coarsePointer) return navEls.current.get(start.topic ?? "look") ?? null
    return (phone && leftTopic.current ? navEls.current.get(view) : null) ?? searchRef.current
  }
  const toTopics = () => {
    leftTopic.current = true
    setStep("topics")
  }

  const common = {
    kind: "form",
    open,
    onClosed,
    // Back with edits has set them aside, which closed the dialog already.
    onClose: (reason: ModalCloseReason) => {
      if (reason !== "back" || unsavedCount === 0) closeCustomizeWebapp()
    },
    beforeClose: (reason: ModalCloseReason) => {
      // Escape over a search clears the search; the next one closes.
      if (reason === "escape" && query !== "") {
        setQuery("")
        return "stay"
      }
      // On the phone's second step the browser's Back is the title row's.
      if (reason === "back" && inTopic) {
        toTopics()
        return "stay"
      }
      return "close"
    },
    // Back cannot stop to ask, so it sets the edits aside for the next opening.
    unsavedEdits: {
      changes: unsavedCount,
      holder: "Preferences has",
      leaves: [chip("config.toml")],
      setAside: () =>
        setAsidePreferencesDraft(
          {
            values: Object.fromEntries(unsavedSettings.map((d) => [d.key, overrides[d.key]])),
            saved: Object.fromEntries(
              unsavedSettings.map((d) => [d.key, editedOver[d.key] ?? originalOf(d)]),
            ),
          },
          unsavedCount,
          passwordTyped,
        ),
    },
    // Closing mid-persist would let the pending success close a freshly
    // reopened dialog, or read as "the edit was discarded" when the request
    // still lands on the server.
    busy: saving,
    size: "w940",
    height: "h680",
    initialFocus,
    submit: {
      label:
        phone && unsavedCount > 0
          ? `Save ${unsavedCount} ${unsavedCount === 1 ? "change" : "changes"}`
          : "Save",
      tone: "primary",
      onSelect: () => void save(),
      busy: saving,
    },
  } as const

  const confirm = (
    <ConfirmResetAll
      names={resetAll?.names ?? null}
      onCancel={() => setResetAll(null)}
      onConfirm={confirmResetAll}
      finalFocus={() => {
        if (!resetAllDone.current) return true
        resetAllDone.current = false
        return (phone ? backRef.current : navEls.current.get("changed")) ?? true
      }}
    />
  )

  if (phone) {
    return (
      <>
        <Modal
          {...common}
          title={
            !inTopic ? "Preferences" : view === "changed" ? CHANGED_LABEL : topicLabel(view)
          }
          step={inTopic ? "topic" : "topics"}
          back={inTopic ? { label: "all topics", onSelect: toTopics } : undefined}
          backRef={backRef}
          toolbar={inTopic ? undefined : searchBox}
          footerNote={partialNotice ?? undefined}
        >
          {inTopic ? (
            viewBody
          ) : searching ? (
            searchResults
          ) : (
            <div className="flex flex-col">
              {nav}
              <div aria-hidden className="my-1.5 h-px shrink-0 bg-border" />
              {openConfigButton}
            </div>
          )}
        </Modal>
        {confirm}
      </>
    )
  }

  return (
    <>
      <Modal
        {...common}
        title="Preferences"
        description={
          <>
            Saved to <InlineCode>config.toml</InlineCode> and applied to every connected
            browser.
          </>
        }
        footerNote={
          <span aria-live="polite">
            {unsavedCount > 0 ? unsavedChangesLabel(unsavedCount) : null}
          </span>
        }
      >
        <div className="grid min-h-0 flex-1 grid-cols-[13.75rem_minmax(0,1fr)] grid-rows-[minmax(0,1fr)] gap-6">
          <div className="flex min-h-0 flex-col gap-3">
            {searchBox}
            {/* The 3px of padding is the room a topic's focus ring needs inside
                the clipping scroll box. */}
            <div className="-m-[3px] min-h-0 shrink overflow-y-auto p-[3px]">{nav}</div>
            <div className="flex-1" />
            {openConfigButton}
          </div>
          {/* The 3px of padding on the content is the room a control's focus ring
              needs inside the clipping viewport; -ml and the 9px right padding
              (12px less the 3px) give it back, so the rows stay in line with the
              heading and the scrollbar keeps its place. Keyed by what is shown,
              so a new view starts at its top. */}
          <ScrollArea
            key={searching ? "search" : view}
            className="-ml-[3px] h-full min-h-0 pr-[9px]"
          >
            <div className="flex min-h-full flex-col px-[3px] pb-[3px]">
              {searching ? searchResults : viewBody}
            </div>
          </ScrollArea>
        </div>
        {partialNotice}
      </Modal>
      {confirm}
    </>
  )
}

// "Reset all" puts the listed settings back in the draft only; Save writes it.
// A sibling of the form's modal, never inside it: a dialog nested in another
// draws no overlay of its own.
function ConfirmResetAll({
  names,
  onCancel,
  onConfirm,
  finalFocus,
}: {
  names: string[] | null
  onCancel: () => void
  onConfirm: () => void
  finalFocus: () => HTMLElement | boolean
}) {
  // Kept through the close, so the closing confirm still says what it said.
  const [shown, setShown] = useState<string[]>(names ?? [])
  if (names !== null && names !== shown) setShown(names)
  const count = shown.length
  return (
    <Modal
      kind="confirm"
      open={names !== null}
      onClose={onCancel}
      title={
        count === 1
          ? "Reset 1 setting to its default?"
          : `Reset ${count} settings to their defaults?`
      }
      description={
        <>
          {listNames(shown)} {count === 1 ? "goes" : "go"} back to what dux ships
          with. <InlineCode>config.toml</InlineCode> is written when you save.
        </>
      }
      cancelLabel="Cancel"
      confirm={{
        label: count === 1 ? "Reset 1 setting" : `Reset ${count} settings`,
        tone: "destructive",
        onSelect: onConfirm,
      }}
      finalFocus={finalFocus}
    />
  )
}

// Topics beside their settings on a computer; on a phone, a topic's settings
// are the second step of the same cover. Every edit and reset waits for Save.
export function CustomizeWebappDialog() {
  const { customizeWebappOpen } = useDux()
  const life = useModalLifetime(customizeWebappOpen)
  // Keyed by the open, so each one starts from the draft that opening hands it.
  return life.mounted ? (
    <PreferencesForm key={life.epoch} open={customizeWebappOpen} onClosed={life.onClosed} />
  ) : null
}
