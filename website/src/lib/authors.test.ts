import { describe, it, expect } from "vitest";
import { getAuthor } from "./authors";

describe("getAuthor", () => {
  it("links a known author to their site", () => {
    expect(getAuthor("Patrick D'appollonio")).toEqual({
      name: "Patrick D'appollonio",
      url: "https://www.patrickdap.com",
    });
  });

  it("returns a bare name (no link) for an unknown author", () => {
    expect(getAuthor("Guest Writer")).toEqual({ name: "Guest Writer" });
  });
});
