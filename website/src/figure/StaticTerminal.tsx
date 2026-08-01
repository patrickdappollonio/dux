// THE ONE STYLISED PIECE OF THE FIGURE, and it is stylised because it cannot be
// anything else.
//
// Everything else on this page is the real dux component, rendered from
// `crates/dux-web/web/src`. The terminal INTERIOR is not, and no amount of work
// will make it so: the real pane is xterm.js, which needs a live DOM to measure
// a character cell against and a live WebSocket streaming PTY bytes to have
// anything to draw. A build-time render has neither. So this block stands in for
// the pane's contents, in the same idiom the rest of the site already uses to
// draw a terminal.
//
// This is not a gap waiting to be closed. Do not "fix" it by pulling xterm in or
// by hydrating this figure: the whole point of the figure is that it ships zero
// JavaScript. The chrome AROUND this block (the tab strip, the PR lane, the
// header, the sidebar, the changed-files pane) is the genuine article.
//
// The transcript is invented, and matches the fabricated workspace in
// `workspace.ts`: an agent partway through adding a payment retry.

interface Line {
  /** Rendered dim, as a shell/agent prompt or chrome. */
  chrome?: boolean
  /** Rendered in the accent colour, as the agent's own voice. */
  accent?: boolean
  text: string
}

const TRANSCRIPT: Line[] = [
  { chrome: true, text: "> retry a failed authorization once before we surface it" },
  { text: "" },
  { accent: true, text: "● Reading the payment intent path" },
  { chrome: true, text: "  └ src/checkout/payment-intent.ts (412 lines)" },
  { text: "" },
  { accent: true, text: "● Added src/checkout/retry-policy.ts" },
  {
    chrome: true,
    text: "  └ one retry, 800ms backoff, only for issuer soft declines",
  },
  { text: "" },
  { accent: true, text: "● Running the checkout suite" },
  { chrome: true, text: "  └ npm test -- src/checkout" },
  { text: "" },
  { text: "  PASS  src/checkout/__tests__/payment-intent.test.ts" },
  { text: "  PASS  src/checkout/__tests__/retry-policy.test.ts" },
  { chrome: true, text: "  Tests: 41 passed, 41 total" },
  { text: "" },
  {
    text: "  A soft decline now retries once. A hard decline still fails fast,",
  },
  { text: "  which is the case the old code was collapsing into the same path." },
  { text: "" },
  { chrome: true, text: "> _" },
]

export function StaticTerminal() {
  return (
    <div className="h-full min-h-0 overflow-hidden bg-background px-4 py-3">
      <pre className="font-mono text-[11px] leading-[1.65] whitespace-pre">
        {TRANSCRIPT.map((line, i) => (
          <div
            key={i}
            className={
              line.accent
                ? "text-primary"
                : line.chrome
                  ? "text-muted-foreground"
                  : "text-foreground"
            }
          >
            {line.text === "" ? " " : line.text}
          </div>
        ))}
      </pre>
    </div>
  )
}
