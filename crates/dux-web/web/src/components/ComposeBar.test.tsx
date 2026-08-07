// @vitest-environment jsdom
import { useState } from "react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import { ComposeBar } from "./ComposeBar"

// The bar is CONTROLLED (the buffer lives in TerminalPane so a pref-flip or
// rotation unmount can't destroy in-progress text), so the tests mount it
// under a tiny stateful harness that plays the parent's role. `onSend` returns
// a boolean, success clears the buffer, failure keeps it, so the mock's return
// value is part of each test's setup.
const onSend = vi.fn<(text: string) => boolean>(() => true)

function Harness({ initial = "" }: { initial?: string }) {
  const [value, setValue] = useState(initial)
  return <ComposeBar value={value} onChange={setValue} onSend={onSend} />
}

beforeEach(() => {
  onSend.mockClear()
  onSend.mockReturnValue(true)
})

afterEach(() => {
  cleanup()
})

function textarea(): HTMLTextAreaElement {
  return screen.getByRole("textbox", {
    name: "Message",
  }) as HTMLTextAreaElement
}

function sendButton(): HTMLElement {
  return screen.getByRole("button", { name: "Send" })
}

describe("ComposeBar", () => {
  it("Enter inserts a newline in the buffer and never calls onSend", () => {
    render(<Harness />)
    const ta = textarea()
    fireEvent.change(ta, { target: { value: "hello" } })
    // Enter is native textarea behavior (a newline), so the component must not
    // intercept it: no preventDefault, no send.
    const notPrevented = fireEvent.keyDown(ta, { key: "Enter" })
    expect(notPrevented).toBe(true)
    fireEvent.keyUp(ta, { key: "Enter" })
    expect(onSend).not.toHaveBeenCalled()
  })

  it("Send fires onSend with the buffer and clears it on success", () => {
    render(<Harness />)
    const ta = textarea()
    fireEvent.change(ta, { target: { value: "run the tests\nplease" } })
    fireEvent.pointerDown(sendButton())
    expect(onSend).toHaveBeenCalledTimes(1)
    expect(onSend).toHaveBeenCalledWith("run the tests\nplease")
    expect(ta.value).toBe("")
  })

  it("keeps the buffer when onSend reports failure", () => {
    // A refused send (not the owner, socket down, oversized) must not destroy
    // the user's message: the parent toasts why, the text stays for a retry.
    onSend.mockReturnValue(false)
    render(<Harness />)
    const ta = textarea()
    fireEvent.change(ta, { target: { value: "precious draft" } })
    fireEvent.pointerDown(sendButton())
    expect(onSend).toHaveBeenCalledWith("precious draft")
    expect(ta.value).toBe("precious draft")
  })

  it("Send on an empty buffer fires onSend with the empty string", () => {
    // Empty Send means "press Enter" (confirm a TUI prompt), so the button must
    // stay enabled and the callback must still fire.
    render(<Harness />)
    expect(sendButton().hasAttribute("disabled")).toBe(false)
    fireEvent.pointerDown(sendButton())
    expect(onSend).toHaveBeenCalledWith("")
  })

  it("preventDefaults the Send pointerdown so focus never leaves the textarea", () => {
    render(<Harness />)
    const ta = textarea()
    ta.focus()
    fireEvent.change(ta, { target: { value: "hi" } })
    // The REAL check: fireEvent returns false iff preventDefault was called,
    // which is what stops the browser moving focus to the button on press.
    const prevented = !fireEvent.pointerDown(sendButton())
    expect(prevented).toBe(true)
    // Belt and braces only: jsdom never moves focus on pointerdown, so this
    // cannot fail on its own; the preventDefault assertion above is the guard.
    expect(document.activeElement).toBe(ta)
  })

  it("a keyboard activation (click with detail 0) sends exactly once", () => {
    // Enter/Space on the focused button fire a click with `detail === 0` and
    // no preceding pointerdown; keyboard and AT users must be able to send.
    render(<Harness />)
    const ta = textarea()
    fireEvent.change(ta, { target: { value: "via keyboard" } })
    fireEvent.click(sendButton(), { detail: 0 })
    expect(onSend).toHaveBeenCalledTimes(1)
    expect(onSend).toHaveBeenCalledWith("via keyboard")
  })

  it("a pointer tap (pointerdown then click with detail > 0) sends exactly once", () => {
    // The pointerdown path already sent; the click that follows a real tap
    // carries detail >= 1 and must be ignored or every tap double-sends.
    render(<Harness />)
    const ta = textarea()
    fireEvent.change(ta, { target: { value: "via tap" } })
    fireEvent.pointerDown(sendButton())
    fireEvent.click(sendButton(), { detail: 1 })
    expect(onSend).toHaveBeenCalledTimes(1)
  })

  it("labels both controls for assistive tech", () => {
    render(<Harness />)
    expect(textarea()).toBeTruthy()
    expect(sendButton()).toBeTruthy()
  })

  it("enables native autocorrect, autocapitalize, and spellcheck on the textarea", () => {
    // The whole point of the compose bar: unlike xterm's hidden textarea (which
    // forces all of these OFF), this input wants the phone keyboard's help.
    render(<Harness />)
    const ta = textarea()
    expect(ta.getAttribute("autocorrect")).toBe("on")
    expect(ta.getAttribute("autocapitalize")).toBe("sentences")
    expect(ta.getAttribute("spellcheck")).toBe("true")
  })

  it("shows the typing hint as the placeholder", () => {
    render(<Harness />)
    expect(textarea().getAttribute("placeholder")).toBe("Type a command…")
  })

  it("renders the value the parent passes (controlled input)", () => {
    render(<Harness initial="carried over" />)
    expect(textarea().value).toBe("carried over")
  })

  // Autosize under Tailwind preflight's `box-sizing: border-box`: the height
  // style covers content + padding + BORDER, but `scrollHeight` excludes the
  // border. Setting height = scrollHeight therefore leaves the content area
  // short by the border width, and with overflow-y hidden that clips the
  // bottom of the last line (the on-device "text cut off at the bottom" bug).
  // jsdom does no layout, so the probe stubs the element's metrics and pins
  // the arithmetic itself.
  it("sizes the textarea to scrollHeight PLUS the border delta (border-box)", () => {
    render(<Harness />)
    const ta = textarea()
    // 60px of content+padding, and a 2px total vertical border
    // (offsetHeight - clientHeight), the textarea's real border-1 on each edge.
    Object.defineProperty(ta, "scrollHeight", { value: 60, configurable: true })
    Object.defineProperty(ta, "offsetHeight", { value: 52, configurable: true })
    Object.defineProperty(ta, "clientHeight", { value: 50, configurable: true })
    fireEvent.change(ta, { target: { value: "one\ntwo" } })
    expect(ta.style.height).toBe("62px")
    expect(ta.style.overflowY).toBe("hidden")
  })

  it("caps the height at MAX_ROWS of content plus padding and border, then scrolls", () => {
    render(<Harness />)
    const ta = textarea()
    // Content far past the cap. jsdom reports no parseable line-height or
    // padding, so the cap resolves to 3 lines * the 20px fallback + 0 padding
    // + the 2px border = 62px; the box must still cover its own border.
    Object.defineProperty(ta, "scrollHeight", { value: 400, configurable: true })
    Object.defineProperty(ta, "offsetHeight", { value: 52, configurable: true })
    Object.defineProperty(ta, "clientHeight", { value: 50, configurable: true })
    fireEvent.change(ta, { target: { value: "a\nb\nc\nd\ne\nf\ng\nh" } })
    expect(ta.style.height).toBe("62px")
    expect(ta.style.overflowY).toBe("auto")
  })

  it("matches the terminal's 14px type, not the browser-default 16px", () => {
    // The xterm canvas next door renders at fontSize 14 (see TerminalPane's
    // Terminal options); text-base (16px) visibly towers over it on a phone.
    render(<Harness />)
    const ta = textarea()
    expect(ta.className).toContain("text-sm")
    expect(ta.className).not.toContain("text-base")
  })
})

describe("ComposeBar restore-bars button", () => {
  it("is absent unless the parent says a bar is hidden", () => {
    render(<Harness />)
    expect(
      screen.queryByRole("button", { name: "Show hidden bars" }),
    ).toBeNull()
  })

  it("renders beside the textarea and fires onRestoreBars", () => {
    const onRestoreBars = vi.fn()
    render(
      <ComposeBar
        value=""
        onChange={() => {}}
        onSend={onSend}
        showRestoreBars
        onRestoreBars={onRestoreBars}
      />,
    )
    const btn = screen.getByRole("button", { name: "Show hidden bars" })
    fireEvent.click(btn)
    expect(onRestoreBars).toHaveBeenCalledTimes(1)
    // Restoring is not sending: the tap must not fire the buffer.
    expect(onSend).not.toHaveBeenCalled()
  })
})
