import { describe, expect, it } from "vitest"

import { LIT_CLASS, TAGLINE, WORD_ROOT_MARGIN, initTagline, splitWords, taglineLines } from "./tagline"

// The same hand-rolled DOM reveal.test.ts uses, cut to what initTagline touches.
// Neither jsdom nor happy-dom is installed here, and a dependency bought to
// exercise a class list and one querySelectorAll is a poor trade.
class FakeElement {
  readonly classes = new Set<string>()
  readonly attributes = new Map<string, string>()
  readonly children: FakeElement[] = []

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

  querySelectorAll(selector: string): FakeElement[] {
    const wanted = selector.slice(1)
    return this.children.filter((child) => child.classes.has(wanted))
  }
}

function taglineSection(words: string[]): FakeElement {
  const el = new FakeElement()
  el.setAttribute("data-tagline", "")
  words.forEach((word) => {
    const span = new FakeElement()
    span.classList.add("tagline-word")
    span.attributes.set("data-word", word)
    el.children.push(span)
  })
  return el
}

function fakeDoc(sections: FakeElement[]) {
  return {
    querySelectorAll: (selector: string) => {
      const attribute = selector.slice(1, -1)
      return sections.filter((el) => el.hasAttribute(attribute))
    },
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

function fakeView(options: { reduced?: boolean; observer?: boolean } = {}) {
  FakeObserver.built = []
  return {
    matchMedia: (query: string) => ({
      matches: options.reduced === true && query.includes("reduce"),
    }),
    IntersectionObserver: options.observer === false ? undefined : FakeObserver,
  } as unknown as Window & typeof globalThis
}

function litWords(section: FakeElement): string[] {
  return section.children
    .filter((child) => child.classes.has(LIT_CLASS))
    .map((child) => child.attributes.get("data-word") ?? "")
}

describe("splitWords", () => {
  it("splits a plain sentence in reading order", () => {
    expect(splitWords("Five agents on five branches")).toEqual([
      "Five",
      "agents",
      "on",
      "five",
      "branches",
    ])
  })

  it("keeps punctuation attached to its word", () => {
    expect(splitWords("all visible at once.")).toEqual(["all", "visible", "at", "once."])
  })

  it("collapses runs of whitespace instead of minting empty words", () => {
    expect(splitWords("  you   stop\tbabysitting \n terminals ")).toEqual([
      "you",
      "stop",
      "babysitting",
      "terminals",
    ])
  })

  it("yields nothing for a blank line", () => {
    expect(splitWords("")).toEqual([])
    expect(splitWords("   \n  ")).toEqual([])
  })
})

describe("taglineLines", () => {
  it("splits every line and keeps the plain text beside it", () => {
    const lines = taglineLines(["one two", "three"])
    expect(lines).toEqual([
      { text: "one two", words: ["one", "two"] },
      { text: "three", words: ["three"] },
    ])
  })

  it("rejoining a line's words reproduces the sentence", () => {
    taglineLines().forEach((line) => {
      expect(line.words.join(" ")).toBe(line.text)
    })
  })

  it("ships the two lines the section renders", () => {
    expect(taglineLines()).toHaveLength(2)
    expect(TAGLINE[0]).toMatch(/five branches/)
    expect(TAGLINE[1]).toMatch(/reviewing work/)
  })
})

describe("initTagline", () => {
  it("lights words in reading order as their entries intersect", () => {
    const section = taglineSection(["Five", "agents", "on", "five", "branches"])
    initTagline(fakeDoc([section]), fakeView())

    const observer = FakeObserver.built[0]
    expect(observer.observed).toEqual(section.children)
    expect(litWords(section)).toEqual([])

    observer.callback([{ isIntersecting: true, target: section.children[0] }])
    expect(litWords(section)).toEqual(["Five"])

    observer.callback([
      { isIntersecting: true, target: section.children[1] },
      { isIntersecting: false, target: section.children[3] },
      { isIntersecting: true, target: section.children[2] },
    ])
    expect(litWords(section)).toEqual(["Five", "agents", "on"])

    expect(observer.unobserved).toEqual(section.children.slice(0, 3))
  })

  it("watches the top half of the viewport, so no word can be skipped", () => {
    // A symmetric band would let a fast scroll jump a word clean over it and
    // leave that word muted for good. The top inset has to stay at zero.
    initTagline(fakeDoc([taglineSection(["one", "two"])]), fakeView())
    expect(FakeObserver.built[0].init.rootMargin).toBe(WORD_ROOT_MARGIN)
    expect(WORD_ROOT_MARGIN).toBe("0px 0px -50% 0px")
  })

  it("lights every word at once and builds no observer under reduced motion", () => {
    const section = taglineSection(["one", "two", "three"])
    initTagline(fakeDoc([section]), fakeView({ reduced: true }))

    expect(litWords(section)).toEqual(["one", "two", "three"])
    expect(FakeObserver.built).toEqual([])
  })

  it("lights every word when the browser has no IntersectionObserver", () => {
    const section = taglineSection(["one", "two"])
    initTagline(fakeDoc([section]), fakeView({ observer: false }))

    expect(litWords(section)).toEqual(["one", "two"])
  })

  it("observes each word once even when two bundles call it", () => {
    const section = taglineSection(["one", "two"])
    const doc = fakeDoc([section])
    const view = fakeView()

    initTagline(doc, view)
    initTagline(doc, view)

    expect(FakeObserver.built).toHaveLength(1)
    expect(FakeObserver.built[0].observed).toEqual(section.children)
  })
})
