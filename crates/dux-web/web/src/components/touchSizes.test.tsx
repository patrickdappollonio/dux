// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import { FileDiskBanner } from "./FileDiskBanner"
import { StoppedAgentBand } from "./StoppedAgentParts"
import {
  SidebarMenuButton,
  SidebarProvider,
  SidebarTrigger,
} from "@/components/ui/sidebar"
import type { AgentTabView } from "@/lib/types"

afterEach(cleanup)

// jsdom cannot evaluate media queries, so the classes are what can be pinned.
const classesOf = (el: Element) => Array.from(el.classList)

describe("touch sizes of the sidebar's own controls", () => {
  it("lifts the collapse button to 40px under a coarse pointer and at phone width", () => {
    render(
      <SidebarProvider>
        <SidebarTrigger />
      </SidebarProvider>,
    )
    expect(
      classesOf(screen.getByRole("button", { name: "Toggle Sidebar" })),
      "the 28px collapse button is under the 40px floor on a touch tablet",
    ).toEqual(expect.arrayContaining(["max-md:size-10", "pointer-coarse:size-10"]))
  })

  it("makes the rail's buttons 40px and centres their icon under a coarse pointer", () => {
    render(
      <SidebarProvider>
        <SidebarMenuButton>x</SidebarMenuButton>
      </SidebarProvider>,
    )
    expect(
      classesOf(screen.getByRole("button")),
      "a rail button stays 32px under a finger, or its icon sits left in the wider box",
    ).toEqual(
      expect.arrayContaining([
        "group-data-[collapsible=icon]:pointer-coarse:size-10!",
        "group-data-[collapsible=icon]:pointer-coarse:justify-center",
      ]),
    )
  })

  it("widens the rail to 56px under a coarse pointer through the class, not an inline style", () => {
    const { container } = render(
      <SidebarProvider>
        <span />
      </SidebarProvider>,
    )
    const wrapper = container.querySelector(
      '[data-slot="sidebar-wrapper"]',
    ) as HTMLElement
    expect(wrapper.style.getPropertyValue("--sidebar-width-icon")).toBe("")
    expect(classesOf(wrapper)).toEqual(
      expect.arrayContaining([
        "[--sidebar-width-icon:3rem]",
        "pointer-coarse:[--sidebar-width-icon:3.5rem]",
      ]),
    )
  })
})

describe("stopped agent band", () => {
  const tab = {
    id: "t1",
    provider: "claude",
    last_run_failed: true,
    last_run_verdict: { ending: "exited", excerpt: ["boom"], ended_at: 0 },
  } as unknown as AgentTabView

  it("keeps the 28px buttons and gives each a 7px margin above and below, a 40px tap area across the 1px border (a browser must measure 40px), that cannot overlap its neighbour", () => {
    render(<StoppedAgentBand sessionId="s1" slotTab={tab} withStart />)
    for (const name of ["Last output", "Start"]) {
      const classes = classesOf(screen.getByRole("button", { name }))
      expect(classes, `${name} keeps its visible height`).toContain("h-7")
      expect(classes, `${name} needs a 40px tap area`).toEqual(
        expect.arrayContaining([
          "relative",
          "after:absolute",
          "after:-inset-y-[7px]",
          "after:inset-x-0",
          "after:hidden",
          "max-md:after:block",
          "pointer-coarse:after:block",
        ]),
      )
      expect(classes, `${name} must not grow its tap area sideways`).not.toContain(
        "after:-inset-[7px]",
      )
    }
  })
})

describe("file disk banner", () => {
  it("lifts every action to 40px at phone width and under a coarse pointer", () => {
    render(
      <FileDiskBanner
        state="changed"
        path="a.ts"
        dirty
        onReload={() => {}}
        onDismiss={() => {}}
        onCloseTab={() => {}}
      />,
    )
    for (const name of ["Reload from disk", "Keep mine"]) {
      expect(
        classesOf(screen.getByRole("button", { name })),
        `${name} is 28px under a finger`,
      ).toEqual(expect.arrayContaining(["max-md:min-h-10", "pointer-coarse:min-h-10"]))
    }
  })
})
