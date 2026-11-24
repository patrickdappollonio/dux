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

// Renders a CPU figure (a percentage of the whole machine) to one decimal, the same string
// the TUI's `format_cpu_percent` prints for the same sample. The server's figure is an f32
// that travels as its shortest decimal (`0.15`), so both surfaces round that decimal text,
// half up, rather than a binary value: `toFixed` would round the double behind 0.15 and
// 12.15 down and the f32's exact expansion rounds 0.25 down, and the two would disagree.
// `cpu_percent_cross_language.json` pins both halves. A figure above zero that would round
// to 0.0 reads "<0.1%" so a small load never looks like none; only exactly zero reads
// "0.0%". Never clamp it at 100: a tree can legitimately read past it (a fractional CPU
// quota is rounded down to whole cores) and clamping would hide that.
export function formatCpu(percent: number): string {
  if (!Number.isFinite(percent) || percent <= 0) return `${percent.toFixed(1)}%`
  if (percent < 0.05) return "<0.1%"
  const decimal = String(percent)
  // JavaScript writes an exponent from 1e21; no CPU figure comes near, but never print one.
  if (decimal.includes("e")) return `${percent.toFixed(1)}%`
  return `${roundDecimalToTenth(decimal)}%`
}

// Rounds a plain positive decimal such as "12.15" to one decimal place, half up, on its
// digits ("12.15" is "12.2", "0.25" is "0.3"). The twin of `round_decimal_to_tenth` in
// dux-core's resource_stats.rs.
function roundDecimalToTenth(decimal: string): string {
  const [whole, fraction = ""] = decimal.split(".")
  const padded = `${fraction}00`
  const digits = `${whole}${padded[0]}`.split("")
  if (padded[1] >= "5") {
    let at = digits.length
    for (;;) {
      if (at === 0) {
        digits.unshift("1")
        break
      }
      at -= 1
      if (digits[at] === "9") {
        digits[at] = "0"
      } else {
        digits[at] = String(Number(digits[at]) + 1)
        break
      }
    }
  }
  const last = digits.pop()
  return `${digits.join("")}.${last}`
}
