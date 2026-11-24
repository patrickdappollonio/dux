// Presentation helpers for the Task Manager's numbers, mirroring the TUI's
// `format_bytes` in `app/render.rs`: both surfaces read the same core sample, so the
// units, thresholds and decimal places must match.

const KIB = 1024
const MIB = KIB * 1024
const GIB = MIB * 1024

// Render a byte count the way the TUI's resource monitor does.
export function formatBytes(bytes: number): string {
  if (bytes >= GIB) return `${(bytes / GIB).toFixed(1)} GiB`
  if (bytes >= MIB) return `${(bytes / MIB).toFixed(1)} MiB`
  if (bytes >= KIB) return `${(bytes / KIB).toFixed(0)} KiB`
  return `${bytes} B`
}

// Renders a CPU figure (a percentage of the whole machine) to one decimal, matching the
// TUI's `format_cpu_percent`. A figure above zero that would round to 0.0 reads "<0.1%"
// so a small load never looks like none; only exactly zero reads "0.0%". Never clamp it at
// 100: sampling noise can nudge a figure past it and clamping would hide that.
export function formatCpu(percent: number): string {
  const text = percent.toFixed(1)
  if (percent > 0 && text === "0.0") return "<0.1%"
  return `${text}%`
}

// What the CPU column counts, said once beside the poll cadence where the table has no room
// for it on every row. The TUI's heading says the same in its own column.
export const CPU_SCOPE_NOTE = "CPU is of all cores"
