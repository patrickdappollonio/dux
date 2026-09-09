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

// Renders a CPU percentage to one decimal, matching the TUI. Never clamp it at 100: a
// process tree with busy threads across several cores legitimately reads higher, and
// clamping would hide the runaway the Task Manager exists to surface.
export function formatCpu(percent: number): string {
  return `${percent.toFixed(1)}%`
}
