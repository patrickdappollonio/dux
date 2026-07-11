import { describe, expect, it } from "vitest"
import {
  osc52SetText,
  osc777Notify,
  osc99Notify,
  osc9IsProgress,
  osc9NotifyBody,
  shouldFireNotification,
} from "./agentNotifications"

describe("osc9 classification", () => {
  it("treats 4;<digits> as progress", () => {
    expect(osc9IsProgress("4;1;50")).toBe(true)
    expect(osc9IsProgress("4;0;0")).toBe(true)
    expect(osc9IsProgress("4;10;5")).toBe(true)
  })

  it("treats prose and non-progress as notifications", () => {
    expect(osc9IsProgress("done")).toBe(false)
    expect(osc9IsProgress("4;hello")).toBe(false)
    expect(osc9NotifyBody("Claude needs your permission")).toBe(
      "Claude needs your permission"
    )
    expect(osc9NotifyBody("4;1;50")).toBeNull()
    expect(osc9NotifyBody("")).toBeNull()
  })
})

describe("osc99 kitty notify", () => {
  it("fires for final displayable notifications", () => {
    expect(osc99Notify(";Build finished")).toEqual({ body: "Build finished" })
    expect(osc99Notify("p=title;Hi")).toEqual({ body: "Hi" })
    expect(osc99Notify("d=1:p=body;Details")).toEqual({ body: "Details" })
  })

  it("does not fire for continuations, control parts, or queries", () => {
    expect(osc99Notify("d=0;partial")).toBeNull()
    expect(osc99Notify("p=close;")).toBeNull()
    expect(osc99Notify("p=?;")).toBeNull()
  })
})

describe("osc777 notify", () => {
  it("parses title and body", () => {
    expect(osc777Notify("notify;Title;Body")).toEqual({
      title: "Title",
      body: "Body",
    })
  })
  it("returns null for non-notify", () => {
    expect(osc777Notify("something;else")).toBeNull()
  })
})

describe("osc52 clipboard", () => {
  it("decodes a SET payload", () => {
    // base64("hello") === "aGVsbG8="
    expect(osc52SetText("c;aGVsbG8=")).toBe("hello")
  })
  it("ignores a read query", () => {
    expect(osc52SetText("c;?")).toBeNull()
  })
  it("ignores a malformed payload", () => {
    expect(osc52SetText("c")).toBeNull()
    expect(osc52SetText("c;")).toBeNull()
  })
})

describe("shouldFireNotification gating", () => {
  const base = {
    enabled: true,
    permission: "granted" as NotificationPermission,
    hidden: true,
    hasFocus: false,
  }
  it("fires only when enabled, granted, and backgrounded", () => {
    expect(shouldFireNotification(base)).toBe(true)
    expect(shouldFireNotification({ ...base, enabled: false })).toBe(false)
    expect(
      shouldFireNotification({ ...base, permission: "default" })
    ).toBe(false)
    // Foregrounded (visible AND focused): suppress.
    expect(
      shouldFireNotification({ ...base, hidden: false, hasFocus: true })
    ).toBe(false)
    // Visible but unfocused still fires.
    expect(
      shouldFireNotification({ ...base, hidden: false, hasFocus: false })
    ).toBe(true)
  })
})
