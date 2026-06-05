// The persistent statusline mirrors the dux TUI's bottom status line 1:1. The
// engine pushes a `StatusTone` (`info` | `busy` | `warning` | `error`) with each
// message; this module is the single source of truth for how the web renders
// each tone. Matching the TUI: Info is plain/muted with no icon, Busy adds the
// braille spinner, Warning is amber with a triangle, Error is red with a circle-x.
//
// Colors reuse the app's established conventions (no new theme tokens):
//   - warning  → text-amber-500  (the AddProjectDialog / branch-warning amber)
//   - error    → text-destructive (the app's prose error color: ChangedFiles,
//                the confirm dialogs)
//   - busy/info → text-muted-foreground (the TUI's plain status tone)

// Tone carried alongside the message in the store.
export interface StatusLineState {
  tone: string
  message: string
}

// Which leading glyph the statusline shows for a tone.
//   - "none"     → no icon (Info)
//   - "spinner"  → the animated BrailleSpinner (Busy)
//   - "warning"  → the TriangleAlert lucide icon (Warning)
//   - "error"    → the CircleX lucide icon (Error)
export type StatusIconKind = "none" | "spinner" | "warning" | "error"

export interface StatusPresentation {
  icon: StatusIconKind
  // Tailwind text-color class applied to BOTH the icon and the message so the
  // tone reads as one colored unit.
  className: string
}

// Map an engine StatusTone string to its web presentation. Unknown tones fall
// back to Info (plain/muted) — the TUI treats Info as its neutral default, and
// an unrecognized tone is never worth screaming about.
export function statusPresentation(tone: string): StatusPresentation {
  switch (tone) {
    case "busy":
      return { icon: "spinner", className: "text-muted-foreground" }
    case "warning":
      return { icon: "warning", className: "text-amber-500" }
    case "error":
      return { icon: "error", className: "text-destructive" }
    default:
      // "info" and any unknown tone: the plain/muted default.
      return { icon: "none", className: "text-muted-foreground" }
  }
}
