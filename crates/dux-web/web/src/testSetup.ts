import { configure } from "@testing-library/dom"

// Every sidebar row stacks an inert, aria-hidden clone of its name over the real
// one, so a text query would find each name twice; what it means is the real one.
if (typeof document !== "undefined") {
  configure({ defaultIgnore: "script, style, .agent-name-shimmer" })
}
