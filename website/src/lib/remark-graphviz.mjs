// Renders ```dot fenced blocks in Markdown as diagrams instead of code. Layout
// runs at build time through Graphviz-WASM (see src/lib/graphviz.mjs), so the
// page ships inline SVG and no JavaScript, and everything after the language word
// on the fence line becomes the caption and the SVG's accessible name.
//
// A small manual mdast walk, like remark-admonitions.mjs, avoiding a
// unist-util-visit dependency. The transformer is async because the WASM module
// loads asynchronously; unified awaits it.

import { renderDiagram } from "./graphviz.mjs";

const LANGUAGES = new Set(["dot", "graphviz"]);

function collect(node, found) {
  if (!node || !Array.isArray(node.children)) return;
  for (const child of node.children) {
    if (child.type === "code" && LANGUAGES.has((child.lang || "").toLowerCase())) {
      found.push(child);
    }
    collect(child, found);
  }
}

export default function remarkGraphviz() {
  return async (tree) => {
    const blocks = [];
    collect(tree, blocks);
    if (blocks.length === 0) return;

    // Sequential rather than parallel: Graphviz-WASM is one shared instance and
    // a build has a handful of diagrams, so there is nothing to win by racing.
    for (const node of blocks) {
      const caption = (node.meta || "").trim();
      const html = await renderDiagram(node.value, { caption });
      node.type = "html";
      node.value = html;
      delete node.lang;
      delete node.meta;
    }
  };
}
