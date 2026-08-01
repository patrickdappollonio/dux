import { afterEach, describe, expect, it, vi } from "vitest";
import { fetchJson } from "./remote-json";

// `fetchJson` memoizes per URL and suppresses a repeated warning per LABEL, both
// for the life of the module, so every case here uses fresh values for each.
function stubFetch(impl: () => unknown) {
  vi.stubGlobal("fetch", vi.fn(impl));
}

function spyWarn() {
  return vi.spyOn(console, "warn").mockImplementation(() => {});
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("fetchJson", () => {
  // The degradation itself is the design and is pinned elsewhere. What is
  // pinned HERE is that it is not silent: a hidden counter must be
  // distinguishable from a broken one by reading the build log.
  it("still degrades to null on a bad status", async () => {
    spyWarn();
    stubFetch(() => ({ ok: false, status: 403, statusText: "Forbidden" }));
    await expect(
      fetchJson("https://api.github.com/repos/a/degrade-status", {
        label: "input-degrade-status",
        effect: "The badge is omitted",
      }),
    ).resolves.toBeNull();
  });

  it("warns with the label, the status and the fix when the server refuses", async () => {
    const warn = spyWarn();
    stubFetch(() => ({ ok: false, status: 403, statusText: "Forbidden" }));
    await fetchJson("https://api.github.com/repos/a/warn-403", {
      label: "the star count for a/warn-403",
      effect: "The badge is omitted",
    });
    expect(warn).toHaveBeenCalledTimes(1);
    const msg = warn.mock.calls[0]![0] as string;
    expect(msg).toContain("the star count for a/warn-403");
    expect(msg).toContain("HTTP 403");
    expect(msg).toContain("RATE LIMITING");
    expect(msg).toContain("GH_TOKEN");
  });

  it("warns with the reason when nothing answers", async () => {
    const warn = spyWarn();
    stubFetch(() => {
      throw new Error("getaddrinfo ENOTFOUND api.github.com");
    });
    await expect(
      fetchJson("https://api.github.com/repos/a/warn-offline", {
        label: "the release tag for a/warn-offline",
        effect: "The hero pill renders without a version",
      }),
    ).resolves.toBeNull();
    const msg = warn.mock.calls[0]![0] as string;
    expect(msg).toContain("getaddrinfo ENOTFOUND api.github.com");
    expect(msg).toContain("connectivity");
  });

  // getNpmTotal walks a package's whole lifetime in 17-month windows, so an
  // offline build would otherwise print the same unreachable-host line once per
  // window and bury everything else in the log.
  it("reports one input once, however many requests it takes", async () => {
    const warn = spyWarn();
    stubFetch(() => ({ ok: false, status: 500, statusText: "Server Error" }));
    const input = { label: "one-label-many-urls", effect: "The counter is hidden" };
    for (const n of [1, 2, 3]) {
      await fetchJson(`https://api.npmjs.org/downloads/point/window-${n}`, input);
    }
    expect(warn).toHaveBeenCalledTimes(1);
  });
});
