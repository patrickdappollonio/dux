import { describe, expect, it } from "vitest"

import {
  ARMED_CLASS,
  REVEAL_THRESHOLD,
  VISIBLE_CLASS,
  armDecision,
  initReveal,
  prefersReducedMotion,
  revealDecision,
  revealObserverInit,
  supportsReveal,
} from "./reveal"

// A DOM small enough to hold in your head. This project has neither jsdom nor
// happy-dom installed, and initReveal touches so little of an element (classes,
// two attributes, a rect and containment) that a real DOM would be a dependency
// bought to exercise five methods.
class FakeElement {
  readonly classes = new Set<string>()
  readonly attributes = new Map<string, string>()
  readonly children: FakeElement[] = []
  id = ""
  /** Where the element's top edge sits relative to the viewport. */
  top = 2000

  readonly classList = {
    add: (name: string) => {
      this.classes.add(name)
    },
    contains: (name: string) => this.classes.has(name),
  }

  hasAttribute(name: string): boolean {
    return this.attributes.has(name)
  }

  setAttribute(name: string, value: string): void {
    this.attributes.set(name, value)
  }

  getBoundingClientRect(): { top: number } {
    return { top: this.top }
  }

  contains(other: FakeElement): boolean {
    return other === this || this.children.some((child) => child.contains(other))
  }
}

function section(options: { top?: number; childId?: string } = {}): FakeElement {
  const el = new FakeElement()
  el.setAttribute("data-reveal", "")
  if (options.top !== undefined) el.top = options.top
  if (options.childId) {
    const child = new FakeElement()
    child.id = options.childId
    el.children.push(child)
  }
  return el
}

function descendants(root: FakeElement): FakeElement[] {
  return [root, ...root.children.flatMap(descendants)]
}

function fakeDoc(sections: FakeElement[]) {
  const all = sections.flatMap(descendants)
  return {
    querySelectorAll: (selector: string) => {
      const attribute = selector.slice(1, -1)
      return all.filter((el) => el.hasAttribute(attribute))
    },
    getElementById: (id: string) => all.find((el) => el.id === id) ?? null,
  } as unknown as Document
}

interface FakeEntry {
  isIntersecting: boolean
  target: FakeElement
}

class FakeObserver {
  static built: FakeObserver[] = []
  readonly observed: FakeElement[] = []
  readonly unobserved: FakeElement[] = []

  constructor(
    readonly callback: (entries: FakeEntry[]) => void,
    readonly init: { threshold: number; rootMargin: string },
  ) {
    FakeObserver.built.push(this)
  }

  observe(el: FakeElement): void {
    this.observed.push(el)
  }

  unobserve(el: FakeElement): void {
    this.unobserved.push(el)
  }
}

function fakeView(options: { reduced?: boolean; hash?: string; observer?: boolean } = {}) {
  FakeObserver.built = []
  return {
    innerHeight: 800,
    location: { hash: options.hash ?? "" },
    matchMedia: (query: string) => ({
      matches: options.reduced === true && query.includes("reduce"),
    }),
    IntersectionObserver: options.observer === false ? undefined : FakeObserver,
  } as unknown as Window & typeof globalThis
}

describe("supportsReveal", () => {
  it("is true when the browser has an IntersectionObserver", () => {
    expect(supportsReveal({ IntersectionObserver: function () {} })).toBe(true)
  })

  it("is false when it does not", () => {
    expect(supportsReveal({})).toBe(false)
    expect(supportsReveal({ IntersectionObserver: undefined })).toBe(false)
    expect(supportsReveal({ IntersectionObserver: {} })).toBe(false)
  })
})

describe("prefersReducedMotion", () => {
  it("reads the reduce query", () => {
    const asked: string[] = []
    const match = (query: string) => {
      asked.push(query)
      return { matches: true }
    }
    expect(prefersReducedMotion(match)).toBe(true)
    expect(asked).toEqual(["(prefers-reduced-motion: reduce)"])
  })

  it("is false when the reader asked for nothing", () => {
    expect(prefersReducedMotion(() => ({ matches: false }))).toBe(false)
  })

  it("is false when the browser has no matchMedia at all", () => {
    expect(prefersReducedMotion(undefined)).toBe(false)
  })

  it("falls back to motion rather than throwing on a hostile matcher", () => {
    expect(
      prefersReducedMotion(() => {
        throw new Error("no media queries here")
      }),
    ).toBe(false)
  })
})

describe("revealObserverInit", () => {
  it("uses the shared threshold and leaves a bottom inset", () => {
    const init = revealObserverInit()
    expect(init.threshold).toBe(REVEAL_THRESHOLD)
    expect(init.rootMargin).toBe("0px 0px -8% 0px")
  })

  it("keeps the threshold small enough to fire near a tall section's top", () => {
    expect(REVEAL_THRESHOLD).toBeGreaterThan(0)
    expect(REVEAL_THRESHOLD).toBeLessThan(0.25)
  })
})

describe("revealDecision", () => {
  it("reveals an intersecting section", () => {
    expect(revealDecision({ isIntersecting: true })).toBe("reveal")
  })

  it("waits on one that has not arrived", () => {
    expect(revealDecision({ isIntersecting: false })).toBe("wait")
  })
})

describe("armDecision", () => {
  it("arms a section still below the fold, where hiding it is invisible", () => {
    expect(armDecision({ top: 1200, viewportHeight: 800, isHashTarget: false })).toBe("arm")
  })

  it("settles a section already on screen rather than blinking it out", () => {
    expect(armDecision({ top: 400, viewportHeight: 800, isHashTarget: false })).toBe("settle")
    expect(armDecision({ top: -300, viewportHeight: 800, isHashTarget: false })).toBe("settle")
  })

  it("settles a deep link's target wherever it sits", () => {
    expect(armDecision({ top: 4000, viewportHeight: 800, isHashTarget: true })).toBe("settle")
  })
})

describe("initReveal", () => {
  it("arms a below-the-fold section and reveals it when its entry intersects", () => {
    const below = section({ top: 1500 })
    const doc = fakeDoc([below])
    const view = fakeView()

    initReveal(doc, view)

    expect(below.classList.contains(ARMED_CLASS)).toBe(true)
    expect(below.classList.contains(VISIBLE_CLASS)).toBe(false)

    const observer = FakeObserver.built[0]
    expect(observer.observed).toEqual([below])

    observer.callback([{ isIntersecting: true, target: below }])

    expect(below.classList.contains(VISIBLE_CLASS)).toBe(true)
    expect(observer.unobserved).toEqual([below])
  })

  it("leaves a section that has not arrived alone", () => {
    const below = section({ top: 1500 })
    initReveal(fakeDoc([below]), fakeView())

    const observer = FakeObserver.built[0]
    observer.callback([{ isIntersecting: false, target: below }])

    expect(below.classList.contains(VISIBLE_CLASS)).toBe(false)
    expect(observer.unobserved).toEqual([])
  })

  it("settles a section already in the viewport instead of arming it", () => {
    const onScreen = section({ top: 200 })
    const below = section({ top: 1500 })
    initReveal(fakeDoc([onScreen, below]), fakeView())

    expect(onScreen.classList.contains(ARMED_CLASS)).toBe(false)
    expect(onScreen.classList.contains(VISIBLE_CLASS)).toBe(true)
    expect(FakeObserver.built[0].observed).toEqual([below])
  })

  it("settles the section a deep link points into, so the scroll target holds still", () => {
    const target = section({ top: 3000, childId: "faq" })
    const other = section({ top: 1500 })
    initReveal(fakeDoc([target, other]), fakeView({ hash: "#faq" }))

    expect(target.classList.contains(ARMED_CLASS)).toBe(false)
    expect(target.classList.contains(VISIBLE_CLASS)).toBe(true)
    expect(FakeObserver.built[0].observed).toEqual([other])
  })

  it("settles every section and builds no observer under reduced motion", () => {
    const first = section({ top: 1500 })
    const second = section({ top: 3000 })
    initReveal(fakeDoc([first, second]), fakeView({ reduced: true }))

    expect(first.classList.contains(VISIBLE_CLASS)).toBe(true)
    expect(second.classList.contains(VISIBLE_CLASS)).toBe(true)
    expect(first.classList.contains(ARMED_CLASS)).toBe(false)
    expect(FakeObserver.built).toEqual([])
  })

  it("settles every section when the browser has no IntersectionObserver", () => {
    const only = section({ top: 1500 })
    initReveal(fakeDoc([only]), fakeView({ observer: false }))

    expect(only.classList.contains(VISIBLE_CLASS)).toBe(true)
    expect(only.classList.contains(ARMED_CLASS)).toBe(false)
  })

  it("observes each section once even when two bundles call it", () => {
    const below = section({ top: 1500 })
    const doc = fakeDoc([below])
    const view = fakeView()

    initReveal(doc, view)
    initReveal(doc, view)

    expect(FakeObserver.built).toHaveLength(1)
    expect(FakeObserver.built[0].observed).toEqual([below])
  })
})
