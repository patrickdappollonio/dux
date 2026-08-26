// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, renderHook } from "@testing-library/react"
import { useEffect as reactUseEffect, useState as reactUseState } from "react"

import type { Terminal } from "@xterm/xterm"

import type { PtySocket } from "@/lib/ptySocket"
import { ESC } from "@/lib/termkeys"

import type { OwnershipVerdict } from "./channels"
import type { LiveSettings, TerminalLiveSettings } from "./liveValues"
import {
  focusTypingSurfaceIn,
  typingFocusAllowed,
  typingSurfaceHasFocusIn,
  useInputSurface,
} from "./inputSurface"

const notified: string[] = []
vi.mock("@/lib/notify", () => ({
  notifyError: (message: string) => {
    notified.push(message)
  },
}))

// The compose draft lives in the store now, keyed by target id, so it survives
// the pane remount `reconnect()` performs. The real module touches
// `localStorage` at import time, which this environment does not provide, and
// this suite is about the input surface rather than about the store, so it gets
// the smallest map that behaves like the real slice.
const { drafts, draftListeners } = vi.hoisted(() => ({
  drafts: {} as Record<string, string>,
  draftListeners: [] as (() => void)[],
}))
vi.mock("@/lib/store", () => ({
  useDux: () => {
    // A version counter is enough to re-render on a write; the hook reads the
    // draft through `composeDraft`, not off this object.
    const [, bump] = reactUseState(0)
    reactUseEffect(() => {
      const listener = () => bump((n) => n + 1)
      draftListeners.push(listener)
      return () => {
        draftListeners.splice(draftListeners.indexOf(listener), 1)
      }
    }, [])
    return { composeDrafts: drafts }
  },
  composeDraft: (_state: unknown, id: string) => drafts[id] ?? "",
  peekComposeDraft: (id: string) => drafts[id] ?? "",
  setComposeDraft: (id: string, text: string) => {
    if (text === "") delete drafts[id]
    else drafts[id] = text
    for (const listener of [...draftListeners]) listener()
  },
}))

class TermFake {
  textarea = document.createElement("textarea")
  focusCalls = 0
  scrolls = 0
  clears = 0
  modes = { applicationCursorKeysMode: false, mouseTrackingMode: "none" }
  buffer = { active: { type: "normal" } }
  rows = 24
  element = document.createElement("div")
  focus() {
    this.focusCalls++
    this.textarea.focus()
  }
  scrollToBottom() {
    this.scrolls++
  }
  clearSelection() {
    this.clears++
  }
  scrollPages() {}
}

function decode(b: Uint8Array): string {
  return new TextDecoder().decode(b)
}

function setup(
  opts: { composeActive?: boolean; owner?: boolean; open?: boolean } = {},
) {
  const term = new TermFake()
  document.body.appendChild(term.textarea)
  const compose = document.createElement("textarea")
  document.body.appendChild(compose)
  const sent: string[] = []
  const pty = {
    isOpen: opts.open ?? true,
    sendInput: (b: Uint8Array) => sent.push(decode(b)),
  }
  const live = {
    current: {
      composeActive: opts.composeActive ?? false,
    } as TerminalLiveSettings,
  } as LiveSettings
  let owner = opts.owner ?? true
  const ownership: OwnershipVerdict = {
    read: () => owner,
    write: (v) => {
      owner = v
    },
  }
  const composeInputRef = { current: compose as HTMLTextAreaElement | null }
  const view = renderHook(() =>
    useInputSurface({
      live,
      composeInputRef,
      termRef: { current: term as unknown as Terminal },
      ptyRef: { current: pty as unknown as PtySocket },
      ownership,
      targetId: "pane-1",
    }),
  )
  return { view, term, compose, sent, live, composeInputRef, pty, ownership }
}

beforeEach(() => {
  for (const key of Object.keys(drafts)) delete drafts[key]
  notified.length = 0
})
afterEach(() => {
  document.body.innerHTML = ""
})

describe("the one focus-routing rule", () => {
  it("goes to the compose textarea while the bar is up", () => {
    const { compose, live, composeInputRef, term } = setup({
      composeActive: true,
    })
    focusTypingSurfaceIn({
      live,
      composeInputRef,
      termRef: { current: term as unknown as Terminal },
    })
    expect(document.activeElement).toBe(compose)
  })

  it("falls to xterm's hidden textarea otherwise", () => {
    const { live, composeInputRef, term } = setup({ composeActive: false })
    focusTypingSurfaceIn({
      live,
      composeInputRef,
      termRef: { current: term as unknown as Terminal },
    })
    expect(term.focusCalls).toBe(1)
  })

  it("falls to xterm when the bar is up but its element is gone", () => {
    const { live, term } = setup({ composeActive: true })
    focusTypingSurfaceIn({
      live,
      composeInputRef: { current: null },
      termRef: { current: term as unknown as Terminal },
    })
    expect(term.focusCalls).toBe(1)
  })

  it("answers the keyboard-state question about the SAME surface", () => {
    const { compose, live, composeInputRef, term } = setup({
      composeActive: true,
    })
    const refs = {
      live,
      composeInputRef,
      termRef: { current: term as unknown as Terminal },
    }
    expect(typingSurfaceHasFocusIn(refs)).toBe(false)
    compose.focus()
    expect(typingSurfaceHasFocusIn(refs)).toBe(true)
  })
})

describe("the sticky modifier latches", () => {
  it("write the visible state and the channel together", () => {
    const { view } = setup()
    act(() => view.result.current.toggleCtrl())
    expect(view.result.current.ctrl).toBe(true)
    expect(view.result.current.mods.read()).toEqual({ ctrl: true, alt: false })
  })

  it("prefix ESC for a latched Alt on an accessory sequence, then clear", () => {
    const { view, sent } = setup()
    act(() => view.result.current.toggleAlt())
    act(() => view.result.current.sendSeq("\x1b[A"))
    expect(sent).toEqual([ESC + "\x1b[A"])
    expect(view.result.current.mods.read()).toEqual({
      ctrl: false,
      alt: false,
    })
  })

  it("are CONSUMED by the accessory newline, which never combines with them", () => {
    const { view, sent } = setup()
    act(() => view.result.current.toggleCtrl())
    act(() => view.result.current.sendNewline())
    expect(sent).toEqual(["\n"])
    expect(view.result.current.ctrl).toBe(false)
  })

  it("are NOT consumed by Send: a latch arms the next KEY, and a message is not a key", () => {
    const { view } = setup()
    act(() => view.result.current.toggleCtrl())
    act(() => {
      view.result.current.sendCompose("hello")
    })
    expect(view.result.current.mods.read().ctrl).toBe(true)
  })
})

describe("owner gating", () => {
  it("drops an accessory sequence from a viewer", () => {
    const { view, sent } = setup({ owner: false })
    act(() => view.result.current.sendSeq(ESC))
    expect(sent).toEqual([])
  })

  it("drops an accessory newline from a viewer", () => {
    const { view, sent } = setup({ owner: false })
    act(() => view.result.current.sendNewline())
    expect(sent).toEqual([])
  })

  it("refuses a Send from a viewer, keeps the buffer, and says why", () => {
    const { view } = setup({ owner: false })
    let ok = true
    act(() => {
      ok = view.result.current.sendCompose("hello")
    })
    expect(ok).toBe(false)
    expect(notified).toHaveLength(1)
    expect(notified[0]).toMatch(/Take over/)
  })

  it("refuses a Send over a closed socket and says why", () => {
    const { view } = setup({ open: false })
    let ok = true
    act(() => {
      ok = view.result.current.sendCompose("hello")
    })
    expect(ok).toBe(false)
    expect(notified[0]).toMatch(/Not connected/)
  })
})

describe("Send", () => {
  it("writes the body first and the submitting CR as a DELAYED second write", () => {
    vi.useFakeTimers()
    try {
      const { view, sent } = setup()
      act(() => {
        view.result.current.sendCompose("hello")
      })
      expect(sent).toEqual(["hello"])
      act(() => {
        vi.advanceTimersByTime(1000)
      })
      expect(sent).toEqual(["hello", "\r"])
    } finally {
      vi.useRealTimers()
    }
  })

  it("replays a typed key's landing effects once, with the first write", () => {
    const { view, term } = setup()
    act(() => {
      view.result.current.sendCompose("hello")
    })
    expect(term.scrolls).toBe(1)
    expect(term.clears).toBe(1)
  })

  it("is ONE immediate bare CR when the buffer is empty", () => {
    vi.useFakeTimers()
    try {
      const { view, sent } = setup()
      act(() => {
        view.result.current.sendCompose("")
      })
      expect(sent).toEqual(["\r"])
      act(() => {
        vi.advanceTimersByTime(1000)
      })
      expect(sent).toEqual(["\r"])
    } finally {
      vi.useRealTimers()
    }
  })
})

describe("the draft", () => {
  it("splices inserted text at the caret and leaves nothing on the wire", () => {
    const { view, compose, sent } = setup({ composeActive: true })
    act(() => view.result.current.setComposeText("ab"))
    compose.value = "ab"
    compose.setSelectionRange(1, 1)
    act(() => view.result.current.insertComposeText("X"))
    expect(view.result.current.composeText).toBe("aXb")
    expect(sent).toEqual([])
  })

  it("appends when there is no selection to splice into", () => {
    const { view } = setup({ composeActive: true })
    act(() => view.result.current.setComposeText("ab"))
    act(() => view.result.current.insertComposeText("X"))
    expect(view.result.current.composeText).toContain("X")
  })
})

// INPUT SAFETY. Focusing summons the soft keyboard, so an automatic focus move
// waits for the pane to have RECONCILED rather than for it to merely believe
// something, and it never interrupts an IME composition.
describe("typingFocusAllowed", () => {
  const ready = {
    isOwner: true,
    ownershipConfirmed: true,
    replayApplied: true,
    composing: false,
  }

  it("allows the move once ownership is confirmed and the screen is on", () => {
    expect(typingFocusAllowed(ready)).toBe(true)
  })

  it("refuses while this device is only GUESSING it owns the pty", () => {
    // Before the handshake, `isOwner` is the foreground guess and nothing more.
    expect(typingFocusAllowed({ ...ready, ownershipConfirmed: false })).toBe(false)
  })

  it("refuses for a watcher", () => {
    expect(typingFocusAllowed({ ...ready, isOwner: false })).toBe(false)
  })

  it("refuses until the replay for this attach epoch is on screen", () => {
    // A keyboard over a pane that is still reconciling covers a placeholder.
    expect(typingFocusAllowed({ ...ready, replayApplied: false })).toBe(false)
  })

  it("refuses mid-IME-composition, which moving focus would destroy", () => {
    expect(typingFocusAllowed({ ...ready, composing: true })).toBe(false)
  })
})
