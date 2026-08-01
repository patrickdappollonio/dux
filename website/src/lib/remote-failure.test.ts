import { describe, expect, it, vi } from "vitest";
// @ts-expect-error - plain .mjs helper, no types
import {
  degradationWarning,
  emitDegradation,
  errorReason,
  unexpectedShapeWarning,
} from "./remote-failure.mjs";

// These messages ARE the feature. The build-time lookups behind them all
// degrade to `null` on purpose (they read third-party APIs, and a rate limit
// somewhere else must not break a contributor's build), so the only thing
// standing between a hidden counter and an unexplained one is this text. Each
// case pins the specific thing a reader needs, so a future edit cannot quietly
// reduce these back to "fetch failed".
const GH = "https://api.github.com/repos/owner/repo";

function ghStatus(status: number, hasToken = false): string {
  return degradationWarning({
    label: "the star count for owner/repo",
    url: GH,
    effect: "The badge is omitted",
    status,
    statusText: status === 403 ? "Forbidden" : "",
    hasToken,
  });
}

describe("degradationWarning", () => {
  it("names the input, the endpoint and what the page loses", () => {
    const msg = ghStatus(403);
    expect(msg).toContain("the star count for owner/repo");
    expect(msg).toContain(GH);
    expect(msg).toContain("The badge is omitted");
    expect(msg).toContain("the build continues");
  });

  it("reports the HTTP status when the server answered", () => {
    expect(ghStatus(403)).toContain("HTTP 403 Forbidden");
  });

  // The whole reason this case is called out: every other API on the internet
  // means "you are not allowed" by 403. GitHub means "you are out of requests",
  // and the two have completely different fixes.
  it("says plainly that a GitHub 403 is rate limiting, not permissions", () => {
    const msg = ghStatus(403);
    expect(msg).toContain("RATE LIMITING");
    expect(msg).toContain("not a permissions problem");
  });

  it("points an unauthenticated build at GH_TOKEN, and at what CI does", () => {
    const msg = ghStatus(403);
    expect(msg).toContain("GH_TOKEN");
    expect(msg).toContain(".github/workflows/pages.yml");
  });

  it("does not tell an already-authenticated build to set the token it has", () => {
    const msg = ghStatus(403, true);
    expect(msg).toContain("A token was already in use");
    expect(msg).not.toContain("Set GH_TOKEN to lift it");
  });

  it("treats 429 as rate limiting too", () => {
    expect(ghStatus(429)).toContain("RATE LIMITING");
  });

  // A 403 only means rate limiting on api.github.com. Anywhere else it is the
  // ordinary meaning, and suggesting a GitHub token would be a wrong lead.
  it("does not blame the rate limiter for a non-GitHub 403", () => {
    const msg = degradationWarning({
      label: "all-time npm downloads",
      url: "https://api.npmjs.org/downloads/point/2024-01-01:2025-01-01/pkg",
      effect: "The npm counter is hidden",
      status: 403,
    });
    expect(msg).not.toContain("RATE LIMITING");
    expect(msg).not.toContain("GH_TOKEN");
  });

  // Rate limited, does not exist, and cannot be reached have three different
  // fixes, so the message must not blur them together.
  it("distinguishes a missing resource from a rate limit", () => {
    const msg = ghStatus(404);
    expect(msg).toContain("does not exist");
    expect(msg).toContain("not rate limiting");
    expect(msg).toContain("a token will not change it");
  });

  it("distinguishes no network from both of those", () => {
    const msg = degradationWarning({
      label: "the latest release tag for owner/repo",
      url: GH,
      effect: "The hero pill renders without a version",
      reason: "getaddrinfo ENOTFOUND api.github.com",
    });
    expect(msg).toContain("No response from");
    expect(msg).toContain("getaddrinfo ENOTFOUND api.github.com");
    expect(msg).toContain("connectivity");
    expect(msg).toContain("not rate limiting and not a missing resource");
    expect(msg).not.toContain("HTTP");
  });

  it("names the upstream when it is a server error", () => {
    expect(ghStatus(503)).toContain("upstream service failing");
  });

  it("stays a single line so one skipped input is one log entry", () => {
    expect(ghStatus(403)).not.toContain("\n");
  });
});

describe("errorReason", () => {
  // Node's fetch throws a bare "fetch failed" for every transport problem and
  // puts the part that identifies it on `cause`. Without unwrapping, an offline
  // build and an expired certificate produce the same useless message.
  it("unwraps the cause Node hides behind `fetch failed`", () => {
    const e = Object.assign(new TypeError("fetch failed"), {
      cause: new Error("getaddrinfo EAI_AGAIN api.github.com"),
    });
    expect(errorReason(e)).toBe("fetch failed: getaddrinfo EAI_AGAIN api.github.com");
  });

  it("does not repeat itself when the cause says the same thing", () => {
    const e = Object.assign(new Error("boom"), { cause: new Error("boom") });
    expect(errorReason(e)).toBe("boom");
  });

  it("names the timeout rather than the abort", () => {
    const e = Object.assign(new Error("This operation was aborted"), {
      name: "AbortError",
    });
    expect(errorReason(e, "timed out after 6s")).toBe("timed out after 6s");
  });
});

describe("unexpectedShapeWarning", () => {
  // A 200 with the wrong body is neither a network problem nor a quota one, so
  // it must not send the reader chasing a token or a firewall.
  it("says the request succeeded and that retrying will not help", () => {
    const msg = unexpectedShapeWarning({
      label: "the latest release tag for owner/repo",
      url: `${GH}/releases/latest`,
      effect: "The hero pill renders without a version",
      detail: "the release carried no `tag_name`",
    });
    expect(msg).toContain("answered successfully");
    expect(msg).toContain("the release carried no `tag_name`");
    expect(msg).toContain("retrying will not help");
    expect(msg).not.toContain("GH_TOKEN");
  });
});

describe("emitDegradation", () => {
  // The reason this helper exists rather than a bare console.warn: a degraded
  // deploy is green and its warnings sit inside a collapsed step, so on CI the
  // line has to become an annotation or nobody sees it.
  it("writes a plain warning off CI and a workflow command on it", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
    const log = vi.spyOn(console, "log").mockImplementation(() => {})
    const previous = process.env.GITHUB_ACTIONS

    delete process.env.GITHUB_ACTIONS
    emitDegradation("counter hidden")
    expect(warn).toHaveBeenCalledWith("counter hidden")
    expect(log).not.toHaveBeenCalled()

    process.env.GITHUB_ACTIONS = "true"
    emitDegradation("counter hidden")
    // stdout, not stderr: Actions only parses workflow commands from stdout.
    expect(log).toHaveBeenCalledWith("::warning::counter hidden")

    if (previous === undefined) delete process.env.GITHUB_ACTIONS
    else process.env.GITHUB_ACTIONS = previous
    warn.mockRestore()
    log.mockRestore()
  })

  it("keeps a multi-line message inside one annotation", () => {
    const log = vi.spyOn(console, "log").mockImplementation(() => {})
    const previous = process.env.GITHUB_ACTIONS
    process.env.GITHUB_ACTIONS = "true"

    emitDegradation("first\nsecond")
    // A raw newline would truncate the annotation at the break.
    expect(log).toHaveBeenCalledWith("::warning::first%0Asecond")

    if (previous === undefined) delete process.env.GITHUB_ACTIONS
    else process.env.GITHUB_ACTIONS = previous
    log.mockRestore()
  })
})
