// The Preferences dialog's pure half: how a row's value compares with its
// default and its saved value, what a row's small line says, what the search
// reads, and the write bodies Save sends. `CustomizeWebappDialog.tsx` renders it.

import type { LucideIcon } from "lucide-react"
import { Bell, Bot, Keyboard, Palette, Shield, SquareTerminal } from "lucide-react"

import { haystackHas, normalizeQuery } from "@/lib/agentSearch"
import { inlineCodeDisplayText } from "@/lib/inlineMarkdown"
import {
  SETTING_TOPICS,
  SURFACE_LABEL,
  type SettingDescriptor,
  type SettingTopic,
  type SettingValue,
} from "@/lib/settingsDescriptors"

// What a value counts as when two are compared: an identity field left empty
// is the shipped title or favicon, which is what the server resolves it to.
function comparable(d: SettingDescriptor, v: SettingValue): SettingValue {
  return d.writeTarget === "identity" && v === "" ? d.default : v
}

export function sameValue(d: SettingDescriptor, a: SettingValue, b: SettingValue): boolean {
  return comparable(d, a) === comparable(d, b)
}

// The value a reset drafts: EMPTY for identity fields, so the file follows the
// shipped title and favicon. The server sends the resolved title, so an
// explicit `title = "dux"` reads as the default, offers no reset and stays.
export function resetValue(d: SettingDescriptor): SettingValue {
  return d.writeTarget === "identity" ? "" : d.default
}

// A value as a row names it in words.
export function valueText(d: SettingDescriptor, value: SettingValue): string {
  const v = comparable(d, value)
  switch (d.control.kind) {
    case "bool":
      return v ? "On" : "Off"
    case "number":
      return d.control.unit ? `${v} ${d.control.unit}` : String(v)
    case "enum": {
      const opt = d.control.options.find((o) => o.value === v)
      return opt?.label ?? String(v)
    }
    case "enum-dynamic":
      return String(v)
    case "text":
      return v === "" ? "empty" : String(v)
    case "favicon":
      return v === "" ? "Original" : String(v).charAt(0).toUpperCase() + String(v).slice(1)
    case "password":
      return "no password"
  }
}

// The documented default as a row's small line names it.
export function defaultValueText(d: SettingDescriptor): string {
  return valueText(d, d.default)
}

// What 0 means for a number row that gives it a meaning of its own.
export function zeroText(d: SettingDescriptor): string | null {
  return d.control.kind === "number" && d.control.zeroMeaning
    ? `0 = ${d.control.zeroMeaning}`
    : null
}

/** What the right side shows: the changed settings, or one topic's. */
export type View = "changed" | SettingTopic

export const TOPIC_ICON: Record<SettingTopic, LucideIcon> = {
  look: Palette,
  typing: Keyboard,
  attention: Bell,
  agents: Bot,
  network: Shield,
  tui: SquareTerminal,
}

export const CHANGED_LABEL = "Changed from default"

export function topicLabel(topic: SettingTopic): string {
  return SETTING_TOPICS.find((t) => t.id === topic)?.label ?? topic
}

// A row's small line: where it applies, its default once it differs from it,
// and outside its own topic that topic, the one part the search reads.
export function rowMeta(
  d: SettingDescriptor,
  changed: boolean,
  showTopic: boolean,
): { text: string; searched: boolean }[] {
  const surface = SURFACE_LABEL[d.surface]
  const topic = topicLabel(d.topic)
  const parts = [{ text: surface, searched: showTopic && topic === surface }]
  if (changed) parts.push({ text: `default ${defaultValueText(d)}`, searched: false })
  if (showTopic && topic !== surface) parts.push({ text: topic, searched: true })
  return parts
}

export function settingsLabel(count: number): string {
  return count === 1 ? "1 setting" : `${count} settings`
}

// "A", "A and B", "A, B and C".
export function listNames(names: string[]): string {
  if (names.length <= 1) return names.join("")
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`
}

// A row is shown only when EVERY word of the query is in its label, prose,
// config key, options or topic, never in where it applies or its default.
export function searchWords(query: string): string[] {
  return normalizeQuery(query).split(/\s+/).filter((w) => w !== "")
}

export function rowMatches(fields: string[], words: string[]): boolean {
  return words.every((word) => haystackHas(word, ...fields))
}

// What a row is searched by. A locked row shows its lock sentence in place of
// its description. `extra` is any other sentence the row shows, in its
// backticked source form.
export function settingSearchFields(
  d: SettingDescriptor,
  lock: string | null,
  extra: string[],
): string[] {
  const fields = [
    d.label,
    inlineCodeDisplayText(lock ?? d.description),
    d.key,
    topicLabel(d.topic),
    ...(d.control.kind === "enum" ? d.control.options.map((o) => o.label) : []),
    ...extra.map(inlineCodeDisplayText),
  ]
  const zero = zeroText(d)
  if (zero) fields.push(zero)
  return fields
}

type SettingsGroup = Record<string, SettingValue>

export interface SettingsBody {
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
export function buildWrites(
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
    if (sameValue(d, value, originalOf(d))) continue
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
