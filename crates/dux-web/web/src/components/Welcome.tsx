import { useState } from "react"

// The dux welcome screen, mirroring the TUI's idle agent pane: the braille
// duck over the block-letter logo, with one playful tip underneath. Art is
// ported verbatim from crates/dux-tui/src/app/render.rs (ASCII_LOGO_ALT and
// ASCII_LOGO) so both surfaces share a face.
const DUCK_ART = `⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣀⣤⠤⣄⣀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀
⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠔⠉⠀⠀⢸⢰⢸⢰⢰⠉⠢⡀⠀⠀⠀⠀⠀⠀⠀⠀⠀
⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⡔⢰⠀⢸⢰⢸⢸⢸⢸⢸⢸⢸⢸⢈⢦⠀⠀⠀⠀⠀⠀⠀⠀
⠀⠀⠀⠀⠀⠀⠀⠀⠀⣰⢠⢸⠤⣤⠤⢸⢸⢸⢸⢸⠤⠤⢐⢸⢸⡆⠀⠀⠀⠀⠀⠀⠀
⢠⠀⠀⠀⠀⠀⠀⠀⠀⢹⢸⢸⢸⢸⢸⣤⢰⢲⢤⣄⢸⢸⢸⢸⢸⡇⠀⠀⠀⠀⠀⠀⠀
⠀⠙⡄⠀⠀⠀⠀⠀⠀⠀⣄⢸⢰⣶⣿⣤⣤⣶⣤⣤⣬⣷⡤⢸⣰⠀⠀⠀⠀⠀⠀⠀⠀
⠀⠀⠈⢦⠀⠀⠀⠀⠀⠀⠈⢦⢸⢈⠛⠿⣿⣼⣼⠿⠛⠁⢸⡼⠀⠀⠀⠀⠀⠀⠀⠀⠀
⠀⠀⠸⠉⢻⣍⠉⠤⣀⣠⣤⣾⢸⣿⣿⣿⣶⣾⣾⣿⣿⢸⢸⠓⠤⣄⠀⠀⠀⠀⠀⠀⠀
⠀⠀⠀⠈⠒⠭⣀⢸⢸⢈⢈⢸⢸⢸⠙⠻⢸⢸⢸⠿⠛⢸⢸⢸⢸⢸⢈⠑⢄⠀⠀⠀⠀
⠀⠀⠀⠀⠀⣼⢸⢸⢈⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⣠⠤⠒⠁⢸⣦⠀⠀⠀
⠀⠀⠀⠀⠀⣿⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢩⡂⢸⢸⣀⣴⣿⠀⠀⠀
⠀⠀⠀⠀⠀⠹⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢉⠉⠉⠁⢸⠃⠀⠀⠀
⠀⠀⠀⠀⠀⠀⢳⢨⠘⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⣴⢸⢸⢸⢸⡟⠀⠀⠀⠀
⠀⠀⠀⠀⠀⠀⠀⠳⣀⢠⢨⢘⢘⢸⢸⢸⢸⢸⢸⢸⢸⣤⣿⢸⢸⢸⣀⠛⠀⠀⠀⠀⠀
⠀⠀⠀⠀⠀⠀⠀⠀⠀⠈⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠀⠀⠀⠀⠀⠀⠀⠀`

const TEXT_LOGO = `       ░██
       ░██
 ░████████ ░██    ░██ ░██    ░██
░██    ░██ ░██    ░██  ░██  ░██
░██    ░██ ░██    ░██   ░█████
░██   ░███ ░██   ░███  ░██  ░██
 ░█████░██  ░█████░██ ░██    ░██ `

// Welcome tips: same sassy spirit as the TUI's WELCOME_TIPS, rewritten where
// the original referenced TUI keybindings. Backticked spans render accented.
const TIPS: string[] = [
  "Lost? `⌘K` opens the command palette. Every action lives there, even the ones you forgot existed.",
  "Need every keystroke? The `fullscreen` button on a terminal captures even `Ctrl+T`. Focus mode: activated.",
  "Any CLI tool can be a provider. Just set its `command` in config.toml. No plugins, no adapters.",
  "Each agent gets companion terminals. The `⋯` menu spawns as many as you like.",
  "Tired of writing commit messages? `Generate with AI` in the commit dialog does it for you.",
  "dux remembers which providers you've run on each worktree. Swap away and back, and each one picks up right where you left it.",
  "Click a changed file to read its diff — syntax highlighting included, no checkout required.",
  "Agents keep running when you close this tab. Come back any time; the terminal repaints like you never left.",
  "Drag the sidebar's right edge to resize it. It remembers.",
  "Hover an agent's status icon to see how it's doing: green runs, amber waits, gray is gone.",
]

// Render a tip, highlighting `backticked` spans in the foreground accent
// (the backticks themselves are not shown) — same convention as the TUI.
function TipText({ tip }: { tip: string }) {
  const parts = tip.split("`")
  return (
    <>
      {parts.map((part, i) =>
        i % 2 === 1 ? (
          <span key={i} className="font-medium text-foreground">
            {part}
          </span>
        ) : (
          <span key={i}>{part}</span>
        ),
      )}
    </>
  )
}

export function Welcome() {
  // One tip per visit to the welcome screen (the component remounts whenever
  // the center pane returns to the idle state).
  const [tip] = useState(() => TIPS[Math.floor(Math.random() * TIPS.length)])

  return (
    <div className="flex h-full w-full select-none flex-col items-center justify-center gap-1 overflow-hidden">
      <pre
        aria-hidden
        className="font-mono text-[11px] leading-[1.15] text-amber-500/70"
      >
        {DUCK_ART}
      </pre>
      <pre
        aria-label="dux"
        className="font-mono text-[11px] leading-[1.15] text-muted-foreground"
      >
        {TEXT_LOGO}
      </pre>
      <p className="mt-6 max-w-md px-6 text-center text-sm text-muted-foreground">
        <TipText tip={tip} />
      </p>
    </div>
  )
}
