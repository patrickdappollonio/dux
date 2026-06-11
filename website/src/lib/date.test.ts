import { describe, it, expect } from "vitest";
import { formatDate, isoDate } from "./date";

describe("formatDate", () => {
  it("formats a date as a long human date", () => {
    expect(formatDate(new Date("2026-06-08"))).toBe("June 8, 2026");
  });

  it("formats in UTC so a bare frontmatter date keeps its day", () => {
    // 2026-01-01 parsed as UTC midnight must not slip to Dec 31 on a build
    // machine west of UTC.
    expect(formatDate(new Date("2026-01-01"))).toBe("January 1, 2026");
  });
});

describe("isoDate", () => {
  it("returns a YYYY-MM-DD string", () => {
    expect(isoDate(new Date("2026-06-08"))).toBe("2026-06-08");
  });
});
