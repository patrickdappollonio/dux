import { describe, expect, it } from "vitest"

import { endingSentence, genericEndingSentence, humanizeAgeAgo } from "./tabVerdict"

// These are a PORT of `dux_core::tab_verdict`, and the terminal UI prints the
// Rust originals. The expectations below are copied from that module's own
// tests: if the two ever disagree, one surface is lying about the same run.
describe("humanizeAgeAgo", () => {
  it("reads as prose, at the same boundaries as the Rust helper", () => {
    expect(humanizeAgeAgo(0)).toBe("moments ago")
    expect(humanizeAgeAgo(44)).toBe("moments ago")
    expect(humanizeAgeAgo(45)).toBe("about a minute ago")
    expect(humanizeAgeAgo(120)).toBe("about 2 minutes ago")
    expect(humanizeAgeAgo(3599)).toBe("about an hour ago")
    expect(humanizeAgeAgo(7200)).toBe("about 2 hours ago")
    expect(humanizeAgeAgo(86_400)).toBe("about a day ago")
    expect(humanizeAgeAgo(3 * 86_400)).toBe("about 3 days ago")
  })

  // A clock that has stepped, or a server that rounded oddly, must not produce
  // "about -1 minutes ago".
  it("treats a negative age as no age at all", () => {
    expect(humanizeAgeAgo(-30)).toBe("moments ago")
  })
})

describe("endingSentence", () => {
  it("words every ending with its age, exactly as the terminal UI does", () => {
    expect(
      endingSentence({ ending: "exited", status: 1, ended_seconds_ago: 120, excerpt: [] }),
    ).toBe(
      "Its last run exited with status 1 about 2 minutes ago, so dux didn\u2019t start it again on its own.",
    )
    expect(
      endingSentence({ ending: "exited_unknown", ended_seconds_ago: 0, excerpt: [] }),
    ).toBe(
      "Its last run exited with an unknown status moments ago, so dux didn\u2019t start it again on its own.",
    )
    expect(
      endingSentence({
        ending: "launch_failed",
        error: "no such file",
        ended_seconds_ago: 0,
        excerpt: [],
      }),
    ).toBe("Its last run could not be launched moments ago: no such file")
    expect(
      endingSentence({ ending: "rapid_clean_exit", ended_seconds_ago: 120, excerpt: [] }),
    ).toBe(
      "Its last run ended about 2 minutes ago in under five seconds with status 0, which dux treats as a run that never came up.",
    )
  })

  it("falls back to the generic sentence for a kind it does not know", () => {
    expect(
      endingSentence({ ending: "from-a-newer-server", ended_seconds_ago: 5, excerpt: [] }),
    ).toBe(genericEndingSentence())
  })

  // A status the server did not send must never become "status 0": zero is what
  // a run that SUCCEEDED exits with, and printing it would be the one wrong
  // answer worse than saying nothing specific.
  it("falls back rather than fabricating a status of zero", () => {
    expect(
      endingSentence({ ending: "exited", status: null, ended_seconds_ago: 5, excerpt: [] }),
    ).toBe(genericEndingSentence())
    expect(
      endingSentence({ ending: "exited", ended_seconds_ago: 5, excerpt: [] }),
    ).toBe(genericEndingSentence())
  })

  // The window in the rapid-exit sentence is a MEASUREMENT: `RAPID_EXIT_WINDOW`
  // in crates/dux-core/src/engine/lifecycle.rs, five seconds today. The wire
  // does not carry it, so the literal here is pinned by this test rather than
  // derived; the Rust side builds its sentence from the constant directly.
  it("quotes RAPID_EXIT_WINDOW's five seconds in the rapid-exit sentence", () => {
    expect(
      endingSentence({ ending: "rapid_clean_exit", ended_seconds_ago: 0, excerpt: [] }),
    ).toContain("in under five seconds")
  })

  // The two surfaces must say the SAME thing. They differ on one deliberate
  // point of punctuation (house style is the typographic apostrophe in the
  // browser, ASCII in the terminal), so the comparison normalises that and
  // nothing else. The Rust originals are copied from `tab_verdict.rs`'s own
  // tests, which assert them there.
  it("says word for word what the terminal UI says, apostrophes aside", () => {
    const ascii = (text: string) => text.replace(/\u2019/g, "'")
    expect(
      ascii(
        endingSentence({ ending: "exited", status: 1, ended_seconds_ago: 120, excerpt: [] }),
      ),
    ).toBe(
      "Its last run exited with status 1 about 2 minutes ago, so dux didn't start it again on its own.",
    )
    expect(
      ascii(endingSentence({ ending: "exited_unknown", ended_seconds_ago: 0, excerpt: [] })),
    ).toBe(
      "Its last run exited with an unknown status moments ago, so dux didn't start it again on its own.",
    )
    expect(
      ascii(
        endingSentence({
          ending: "launch_failed",
          error: "no such file",
          ended_seconds_ago: 0,
          excerpt: [],
        }),
      ),
    ).toBe("Its last run could not be launched moments ago: no such file")
    expect(
      ascii(endingSentence({ ending: "rapid_clean_exit", ended_seconds_ago: 120, excerpt: [] })),
    ).toBe(
      "Its last run ended about 2 minutes ago in under five seconds with status 0, which dux treats as a run that never came up.",
    )
  })
})
