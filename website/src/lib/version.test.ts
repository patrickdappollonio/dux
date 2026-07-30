import { afterEach, describe, expect, it, vi } from "vitest";
import { getLatestVersion } from "./version";

// `fetchJson` memoizes per URL for the life of the module, so each case uses a
// distinct repo name to get a fresh lookup.
function stubFetch(body: unknown, ok = true) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => ({ ok, json: async () => body })),
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("getLatestVersion", () => {
  it("returns the release tag", async () => {
    stubFetch({ tag_name: "v0.7.0" });
    await expect(getLatestVersion("owner/good")).resolves.toBe("v0.7.0");
  });

  it("accepts a prerelease-style tag", async () => {
    stubFetch({ tag_name: "v1.0.0-rc.2" });
    await expect(getLatestVersion("owner/rc")).resolves.toBe("v1.0.0-rc.2");
  });

  // The whole point of deriving the version is that a failure shows NOTHING
  // rather than something stale. Every degraded path must yield null so the
  // caller drops the version segment instead of printing a wrong number.
  it("returns null when the lookup fails", async () => {
    stubFetch(null, false);
    await expect(getLatestVersion("owner/failed")).resolves.toBeNull();
  });

  it("returns null when the response has no tag", async () => {
    stubFetch({});
    await expect(getLatestVersion("owner/notag")).resolves.toBeNull();
  });

  it("returns null for a tag that is not version-shaped", async () => {
    stubFetch({ tag_name: "nightly" });
    await expect(getLatestVersion("owner/weird")).resolves.toBeNull();
  });
});
