import { describe, expect, it } from "vitest";
// @ts-expect-error - plain .mjs plugin, no types
import remarkAdmonitions from "./remark-admonitions.mjs";

// Build a minimal mdast root holding one blockquote whose single paragraph is
// `text`. The plugin mutates the tree in place, so we return it for assertions.
function run(text: string): any {
  const tree = {
    type: "root",
    children: [
      {
        type: "blockquote",
        children: [{ type: "paragraph", children: [{ type: "text", value: text }] }],
      },
    ],
  };
  remarkAdmonitions()(tree);
  return tree;
}

describe("remark-admonitions", () => {
  it("turns a [!NOTE] blockquote into a titled note admonition, stripping the marker", () => {
    const bq = run("[!NOTE]\nHeads up.").children[0];

    expect(bq.data.hName).toBe("div");
    expect(bq.data.hProperties.className).toContain("admonition");
    expect(bq.data.hProperties.className).toContain("admonition-note");

    // A title row is prepended, rendered as its own div carrying an icon + label.
    const title = bq.children[0];
    expect(title.data.hName).toBe("div");
    expect(title.data.hProperties.className).toContain("admonition-title");
    expect(
      title.data.hChildren.some((c: any) => c.type === "text" && c.value === "Note"),
    ).toBe(true);
    expect(
      title.data.hChildren.some((c: any) => c.type === "element" && c.tagName === "svg"),
    ).toBe(true);

    // The marker is gone; the content survives.
    expect(bq.children[1].children[0].value).toBe("Heads up.");
  });

  it("leaves an ordinary blockquote untouched", () => {
    const bq = run("Just a quote.").children[0];
    expect(bq.data?.hName).toBeUndefined();
    expect(bq.children[0].type).toBe("paragraph");
  });

  it("recognizes every alert type, case-insensitively", () => {
    const cases: [string, string][] = [
      ["[!TIP]", "tip"],
      ["[!important]", "important"],
      ["[!WARNING]", "warning"],
      ["[!Caution]", "caution"],
    ];
    for (const [marker, type] of cases) {
      const bq = run(`${marker}\nbody`).children[0];
      expect(bq.data.hProperties.className).toContain(`admonition-${type}`);
    }
  });

  it("handles a marker alone on its line with the body in a following paragraph", () => {
    const tree = {
      type: "root",
      children: [
        {
          type: "blockquote",
          children: [
            { type: "paragraph", children: [{ type: "text", value: "[!WARNING]" }] },
            { type: "paragraph", children: [{ type: "text", value: "Careful." }] },
          ],
        },
      ],
    };
    remarkAdmonitions()(tree);
    const bq: any = tree.children[0];
    expect(bq.data.hProperties.className).toContain("admonition-warning");
    // Title, then the body paragraph (the empty marker paragraph is dropped).
    expect(bq.children[0].data.hProperties.className).toContain("admonition-title");
    expect(bq.children[1].children[0].value).toBe("Careful.");
    expect(bq.children).toHaveLength(2);
  });
});
