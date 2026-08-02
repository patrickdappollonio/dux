// Pure helpers backing the flat "Terminals" section of the sidebar, shared by the
// desktop sidebar and the mobile hub so the two surfaces never drift. Kept free
// of React so every rule here is trivially unit-testable.
//
// Since the terminal/agent sidebar parity work, companion terminals are no longer
// nested under their agent row and project terminals are no longer a loose group:
// EVERY terminal (session-owned + project-owned) renders flat in one "Terminals"
// section at the bottom of the list. These helpers own the two data decisions that
// section needs: assembling that flat list with each terminal's owner label, and
// deriving a terminal's state word from its working/typing flags.

import { assertNever } from "@/lib/assertNever"
import type { FlatSortKey, StateWord } from "@/lib/flatList"
import {
  ownerKey,
  ownerRefFromWire,
  type TerminalOwnerRef,
} from "@/lib/terminalOwner"
import type { ProjectView, SessionView, TerminalView } from "@/lib/types"

// One entry in the flat Terminals section: the terminal, its owner reference (so
// a tap selects/streams it), the owner's display label (the agent name, or the
// project name for a project terminal), the project name tag, and the sibling set
// the same owner shares (so `terminalTitle` can disambiguate two terminals running
// the same app).
export interface FlatTerminal {
  terminal: TerminalView
  owner: TerminalOwnerRef
  ownerLabel: string
  projectName: string
  siblings: readonly TerminalView[]
}

// Decorate the spine's flat `terminals` collection with everything the row needs
// that the terminal itself does not carry: its owner reference, the owner's
// display label, the project tag, and its sibling set.
//
// `terminals` arrives flat and owner-tagged, so this walks it ONCE and switches
// on each terminal's own owner. It used to walk `sessions[].terminals` and then
// `projects[].terminals`, which meant ownership was inferred from which loop a
// terminal turned up in and a new kind of owner would simply have had no loop.
// `sessions` and `projects` are now only lookup tables for the labels, so the
// output order is the order of `terminals` (the caller re-sorts into the global
// `sort_order` base either way).
//
// A companion terminal is labeled `agent@project` (the agent's display name --
// title, or branch name when untitled -- at its project); a project terminal
// carries just the project name (it has no agent); a standalone terminal carries
// the `~`-shortened directory it opened in (it has no owner to name at all). An owner id that resolves to
// nothing falls back to the id itself, matching the TUI's sidebar: the spine is
// self-consistent so this should not happen, but showing the row with a truthful
// id beats dropping it, which is the silent omission this shape exists to end.
export function assembleFlatTerminals(
  terminals: readonly TerminalView[],
  sessions: readonly SessionView[],
  projects: readonly ProjectView[],
  projectName: (id: string) => string,
): FlatTerminal[] {
  const sessionsById = new Map(sessions.map((s) => [s.id, s]))
  const projectsById = new Map(projects.map((p) => [p.id, p]))
  // Siblings are the terminals sharing an owner. Nesting used to give this for
  // free (the array a terminal sat in WAS its sibling set); with one flat list
  // it is a grouping pass, keyed so a session and a project with the same id can
  // never merge.
  const byOwner = new Map<string, TerminalView[]>()
  for (const terminal of terminals) {
    const key = ownerKey(terminal.owner)
    const group = byOwner.get(key)
    if (group) group.push(terminal)
    else byOwner.set(key, [terminal])
  }

  const out: FlatTerminal[] = []
  for (const terminal of terminals) {
    const wire = terminal.owner
    const siblings = byOwner.get(ownerKey(wire)) ?? [terminal]
    let ownerLabel: string
    let proj: string
    switch (wire.kind) {
      case "session": {
        const session = sessionsById.get(wire.session_id)
        if (session) {
          proj = projectName(session.project_id)
          ownerLabel = `${session.title || session.branch_name}@${proj}`
        } else {
          proj = ""
          ownerLabel = wire.session_id
        }
        break
      }
      case "project": {
        proj = projectsById.has(wire.project_id)
          ? projectName(wire.project_id)
          : wire.project_id
        ownerLabel = proj
        break
      }
      case "standalone": {
        // No owner to name, so the row's second line names the DIRECTORY the
        // terminal opened in, already shortened with `~` by the server. That is
        // also what the sidebar search matches, which is why it goes in
        // `ownerLabel` rather than somewhere beside it. The project tag is
        // empty, truthfully: it belongs to no project.
        proj = ""
        ownerLabel = wire.cwd_label
        break
      }
      default:
        return assertNever(wire)
    }
    out.push({
      terminal,
      owner: ownerRefFromWire(wire),
      ownerLabel,
      projectName: proj,
      siblings,
    })
  }
  return out
}

// A terminal's colored state word. TWIN of the core-owned
// `dux_core::row_state::terminal_row_state` (the DECISION); pinned by shared
// vectors (`flatTerminals.test.ts` mirrors `row_state.rs`). Only the three states
// a terminal can have (no detached/exited/attention): typing outranks running
// outranks idle. The busy word is "Running" (a terminal runs a process; agents
// say "Working"), but the colors reuse the exact tokens the agent word uses so the
// two never drift: the soft-violet typing token, the active-green busy color,
// muted for idle.
export function terminalStateWord(terminal: TerminalView): StateWord {
  if (terminal.typing) return { label: "Typing", className: "text-dux-typing" }
  if (terminal.working) return { label: "Running", className: "text-green-500" }
  return { label: "Idle", className: "text-muted-foreground" }
}

// A terminal's WYSIWYG name-sort key: the same primary label the row shows,
// `foreground_cmd` when present and non-empty else `label`, lowercased. Using the
// displayed label (not the internal `label`) makes name-sort match what the user
// reads. Mirrors the TUI `terminal_items` name key in `app/mod.rs`.
function terminalNameKey(t: TerminalView): string {
  const cmd = t.foreground_cmd
  return (cmd && cmd.length > 0 ? cmd : t.label).toLowerCase()
}

// Parse an RFC 3339 timestamp to epoch ms, guarding NaN (an empty/unparseable
// value from an older server sorts as 0). Matches `sortSessions.ts`'s `epoch`.
function terminalEpoch(iso: string): number {
  const ms = Date.parse(iso)
  return Number.isNaN(ms) ? 0 : ms
}

// Code-point name comparison, identical in spirit to `sortSessions.ts`'s
// `compareName`: iterate Unicode code points so the order matches Rust's
// `str::cmp` on the lowercased key (the TUI side). Returns <0 / 0 / >0 ascending.
function compareTerminalName(a: TerminalView, b: TerminalView): number {
  const ka = [...terminalNameKey(a)]
  const kb = [...terminalNameKey(b)]
  const len = Math.min(ka.length, kb.length)
  for (let i = 0; i < len; i++) {
    const ca = ka[i].codePointAt(0) ?? 0
    const cb = kb[i].codePointAt(0) ?? 0
    if (ca !== cb) return ca - cb
  }
  return ka.length - kb.length
}

// Order the flat Terminals section for display by the shared sort mode, mirroring
// `sortMainSessions`/`sortedSessionIds` for agents but over the flat terminal list.
// The caller passes the list ALREADY in the global base `sort_order` order (the
// component sorts the assembled list by `sort_order` and applies any optimistic
// drag overlay first), exactly as the agent list relies on `spine.sessions` already
// arriving in `sort_order` order. That is why "manual" is verbatim here rather than
// re-sorting by `sort_order` as the TUI's `terminal_items` does (the TUI reads from
// an unordered HashMap, so it must sort the base itself; the semantic COMPARATORS
// are what stay in lockstep). That base order is the stable starting order every
// branch builds on:
//   - manual  → the base order verbatim (the drag order).
//   - active  → working-or-typing terminals float to the top (a stable float, not
//               a re-sort), each group keeping base order. Default. Terminals have
//               no needs-attention.
//   - updated → newest `updated_at` first (Rust `Reverse(updated_at)`).
//   - created → newest `created_at` first (Rust `Reverse(created_at)`).
//   - name / name_desc → by the DISPLAYED label (WYSIWYG), A to Z / Z to A.
//
// `Array.prototype.sort` is spec-stable, so equal keys keep base order, matching
// the TUI's stable `sort_by_key`. These comparators are kept in LOCKSTEP with the
// TUI `terminal_items` in `crates/dux-tui/src/app/mod.rs` (duplicated per surface
// by necessity). A copy is sorted so the caller's array is untouched.
// The terminal drag baseline, the twin of `displayedSessionOrder` in
// flatList.ts: the COMPLETE flat terminal id list in the order the shared sort
// mode displays it, so a drop made from a computed mode persists what the user
// was looking at (never the hidden base order, and never a search-filtered
// subset; the persisted order is total). Manual needs no special case here:
// `sortFlatTerminals` already returns the base order verbatim for it, which is
// exactly how manual terminal drags always computed their move.
export function displayedTerminalOrder(
  items: FlatTerminal[],
  key: FlatSortKey,
): string[] {
  return sortFlatTerminals(items, key).map((item) => item.terminal.id)
}

export function sortFlatTerminals(
  items: FlatTerminal[],
  key: FlatSortKey,
): FlatTerminal[] {
  const sorted = items.slice()
  switch (key) {
    case "manual":
      // Base order verbatim.
      break
    case "active": {
      // Stable float: hot terminals first (keeping base order), then the rest.
      const hot: FlatTerminal[] = []
      const rest: FlatTerminal[] = []
      for (const item of sorted) {
        if (item.terminal.working || item.terminal.typing) hot.push(item)
        else rest.push(item)
      }
      return [...hot, ...rest]
    }
    case "updated":
      sorted.sort(
        (a, b) => terminalEpoch(b.terminal.updated_at) - terminalEpoch(a.terminal.updated_at),
      )
      break
    case "created":
      sorted.sort(
        (a, b) => terminalEpoch(b.terminal.created_at) - terminalEpoch(a.terminal.created_at),
      )
      break
    case "name":
      sorted.sort((a, b) => compareTerminalName(a.terminal, b.terminal))
      break
    case "name_desc":
      sorted.sort((a, b) => -compareTerminalName(a.terminal, b.terminal))
      break
  }
  return sorted
}
