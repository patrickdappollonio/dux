import { describe, it, expect } from "vitest";
import { normalizeContributors, toSnapshot, avatarPath } from "./contributors-data.mjs";

// A minimal raw record shaped like the GitHub /contributors API response.
function record(overrides = {}) {
  return {
    login: "octocat",
    html_url: "https://github.com/octocat",
    avatar_url: "https://avatars.githubusercontent.com/u/1?v=4",
    type: "User",
    contributions: 10,
    ...overrides,
  };
}

describe("avatarPath", () => {
  it("builds the local served path from a login", () => {
    expect(avatarPath("octocat")).toBe("/contributors/octocat.png");
  });
});

describe("normalizeContributors", () => {
  it("maps a user record to login, profileUrl, avatarUrl and local avatar path", () => {
    expect(normalizeContributors([record()])).toEqual([
      {
        login: "octocat",
        profileUrl: "https://github.com/octocat",
        avatarUrl: "https://avatars.githubusercontent.com/u/1?v=4",
        avatar: "/contributors/octocat.png",
      },
    ]);
  });

  it("drops records whose type is Bot", () => {
    const input = [
      record({ login: "real-user" }),
      record({ login: "dependabot", type: "Bot" }),
    ];
    expect(normalizeContributors(input).map((c) => c.login)).toEqual(["real-user"]);
  });

  it("drops records whose login ends in [bot] even if type is missing", () => {
    const input = [
      record({ login: "real-user" }),
      record({ login: "github-actions[bot]", type: undefined }),
    ];
    expect(normalizeContributors(input).map((c) => c.login)).toEqual(["real-user"]);
  });

  it("drops records missing a login", () => {
    const input = [record(), record({ login: undefined }), record({ login: "" })];
    expect(normalizeContributors(input).map((c) => c.login)).toEqual(["octocat"]);
  });

  it("drops records missing an html_url", () => {
    const input = [record({ login: "a" }), record({ login: "b", html_url: undefined })];
    expect(normalizeContributors(input).map((c) => c.login)).toEqual(["a"]);
  });

  it("drops records missing an avatar_url (nothing to download)", () => {
    const input = [record({ login: "a" }), record({ login: "b", avatar_url: undefined })];
    expect(normalizeContributors(input).map((c) => c.login)).toEqual(["a"]);
  });

  it("preserves input order (API returns contributors by contributions desc)", () => {
    const input = [
      record({ login: "first" }),
      record({ login: "second" }),
      record({ login: "third" }),
    ];
    expect(normalizeContributors(input).map((c) => c.login)).toEqual([
      "first",
      "second",
      "third",
    ]);
  });

  it("returns an empty array for non-array or empty input", () => {
    expect(normalizeContributors(null)).toEqual([]);
    expect(normalizeContributors(undefined)).toEqual([]);
    expect(normalizeContributors([])).toEqual([]);
  });
});

describe("normalizeContributors validation", () => {
  it("drops a login with path-traversal or other invalid characters", () => {
    const input = [
      record({ login: "../evil" }),
      record({ login: "a/b" }),
      record({ login: "has space" }),
      record({ login: "ok-name" }),
    ];
    expect(normalizeContributors(input).map((c) => c.login)).toEqual(["ok-name"]);
  });

  it("drops a record whose html_url is not a github.com profile URL", () => {
    const input = [
      record({ login: "evil", html_url: "javascript:alert(1)" }),
      record({ login: "spoof", html_url: "https://github.com.evil.com/x" }),
      record({ login: "good" }),
    ];
    expect(normalizeContributors(input).map((c) => c.login)).toEqual(["good"]);
  });

  it("drops a record whose avatar_url is not on the GitHub avatar host", () => {
    const input = [
      record({ login: "ssrf", avatar_url: "http://169.254.169.254/latest/meta-data/" }),
      record({ login: "elsewhere", avatar_url: "https://evil.com/a.png" }),
      record({ login: "insecure", avatar_url: "http://avatars.githubusercontent.com/u/1" }),
      record({ login: "good" }),
    ];
    expect(normalizeContributors(input).map((c) => c.login)).toEqual(["good"]);
  });

  it("drops a record with a malformed avatar_url without throwing", () => {
    const input = [record({ login: "bad", avatar_url: "not a url" }), record({ login: "good" })];
    expect(() => normalizeContributors(input)).not.toThrow();
    expect(normalizeContributors(input).map((c) => c.login)).toEqual(["good"]);
  });
});

describe("toSnapshot", () => {
  it("keeps only the fields the committed JSON and component need", () => {
    const normalized = normalizeContributors([record()]);
    expect(toSnapshot(normalized)).toEqual([
      {
        login: "octocat",
        profileUrl: "https://github.com/octocat",
        avatar: "/contributors/octocat.png",
      },
    ]);
  });
});
