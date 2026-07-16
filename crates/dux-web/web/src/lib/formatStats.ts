// Presentation helpers for the Task Manager's numbers.
//
// These mirror the TUI's `format_bytes` (`app/render.rs`) deliberately: both
// surfaces read the same core sample, so a MiB in the TUI must be a MiB in the
// browser. Binary units (KiB/MiB/GiB), same thresholds, same decimal places.
// Formatting is presentation, so it lives here rather than in core: sharing it
// from Rust would need a codegen path that does not exist.

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

// Render a CPU percentage to one decimal, matching the TUI.
//
// NEVER clamp this at 100: a process tree with several busy threads spread
// across cores legitimately reads above 100% (a real measurement on a busy tree
// was 129.5%), and pinning it to 100 would hide exactly the runaway the Task
// Manager exists to surface.
export function formatCpu(percent: number): string {
  return `${percent.toFixed(1)}%`
}
