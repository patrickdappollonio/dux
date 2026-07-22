// GitHub-style admonitions (alerts) for Markdown prose. Runs as a remark plugin
// on the shared markdown processor (see astro.config.mjs), so any `.md`/`.mdx`
// page can use the familiar GitHub syntax:
//
//   > [!NOTE]
//   > Useful information the reader should know.
//
// The recognized types are NOTE, TIP, IMPORTANT, WARNING, and CAUTION. A matching
// blockquote is rewritten to render as `<div class="admonition admonition-note">`
// with a titled header (an octicon + label); the color and layout live in
// global.css. An ordinary blockquote (no `[!TYPE]` first line) is left untouched.
//
// Written as a small manual mdast walk to avoid a unist-util-visit dependency
// (mirrors rehype-prose-images.mjs). The octicon SVG is injected directly as hast
// via `data.hChildren`, so its color follows the title's `currentColor` and no
// CSS data-URI encoding is needed.

const LABEL = {
  note: "Note",
  tip: "Tip",
  important: "Important",
  warning: "Warning",
  caution: "Caution",
};

// The first line of the blockquote must be exactly the marker (GitHub's rule):
// `[!TYPE]` followed by optional trailing spaces/tabs and then a newline or the
// end of the text. `[^\S\n]` is "whitespace but not a newline".
const MARKER = /^\[!(note|tip|important|warning|caution)\][^\S\n]*(?:\n|$)/i;

// GitHub octicon path data (16x16), one per alert type. Injected as hast below.
const ICON_PATH = {
  note: "M0 8a8 8 0 1 1 16 0A8 8 0 0 1 0 8Zm8-6.5a6.5 6.5 0 1 0 0 13 6.5 6.5 0 0 0 0-13ZM6.5 7.75A.75.75 0 0 1 7.25 7h1a.75.75 0 0 1 .75.75v2.75h.25a.75.75 0 0 1 0 1.5h-2a.75.75 0 0 1 0-1.5h.25v-2h-.25a.75.75 0 0 1-.75-.75ZM8 6a1 1 0 1 1 0-2 1 1 0 0 1 0 2Z",
  tip: "M8 1.5c-2.363 0-4 1.69-4 3.75 0 .984.424 1.625.984 2.304l.214.253c.223.264.47.556.673.848.284.411.537.896.621 1.49a.75.75 0 0 1-1.484.211c-.04-.282-.163-.547-.37-.847a8.456 8.456 0 0 0-.542-.68c-.084-.1-.173-.205-.268-.32C3.201 7.75 2.5 6.766 2.5 5.25 2.5 2.31 4.863 0 8 0s5.5 2.31 5.5 5.25c0 1.516-.701 2.5-1.328 3.259-.095.115-.184.22-.268.319-.207.245-.383.453-.541.681-.208.3-.33.565-.37.847a.751.751 0 0 1-1.485-.212c.084-.593.337-1.078.621-1.489.203-.292.45-.584.673-.848.075-.088.147-.173.213-.253.561-.679.985-1.32.985-2.304 0-2.06-1.637-3.75-4-3.75ZM5.75 12h4.5a.75.75 0 0 1 0 1.5h-4.5a.75.75 0 0 1 0-1.5ZM6 15.25a.75.75 0 0 1 .75-.75h2.5a.75.75 0 0 1 0 1.5h-2.5a.75.75 0 0 1-.75-.75Z",
  important:
    "M0 1.75C0 .784.784 0 1.75 0h12.5C15.216 0 16 .784 16 1.75v9.5A1.75 1.75 0 0 1 14.25 13H8.06l-2.573 2.573A1.458 1.458 0 0 1 3 14.543V13H1.75A1.75 1.75 0 0 1 0 11.25Zm1.75-.25a.25.25 0 0 0-.25.25v9.5c0 .138.112.25.25.25h2a.75.75 0 0 1 .75.75v2.19l2.72-2.72a.749.749 0 0 1 .53-.22h6.5a.25.25 0 0 0 .25-.25v-9.5a.25.25 0 0 0-.25-.25Zm7 2.25v2.5a.75.75 0 0 1-1.5 0v-2.5a.75.75 0 0 1 1.5 0ZM8 9a1 1 0 1 1 0-2 1 1 0 0 1 0 2Z",
  warning:
    "M6.457 1.047c.659-1.234 2.427-1.234 3.086 0l6.082 11.378A1.75 1.75 0 0 1 14.082 15H1.918a1.75 1.75 0 0 1-1.543-2.575Zm1.763.707a.25.25 0 0 0-.44 0L1.698 13.132a.25.25 0 0 0 .22.368h12.164a.25.25 0 0 0 .22-.368Zm.53 3.996v2.5a.75.75 0 0 1-1.5 0v-2.5a.75.75 0 0 1 1.5 0ZM9 11a1 1 0 1 1-2 0 1 1 0 0 1 2 0Z",
  caution:
    "M4.47.22A.749.749 0 0 1 5 0h6c.199 0 .389.079.53.22l4.25 4.25c.141.141.22.331.22.53v6a.749.749 0 0 1-.22.53l-4.25 4.25A.749.749 0 0 1 11 16H5a.749.749 0 0 1-.53-.22L.22 11.53A.749.749 0 0 1 0 11V5c0-.199.079-.389.22-.53Zm.84 1.28L1.5 5.31v5.38l3.81 3.81h5.38l3.81-3.81V5.31L10.69 1.5ZM8 4a.75.75 0 0 1 .75.75v3.5a.75.75 0 0 1-1.5 0v-3.5A.75.75 0 0 1 8 4Zm0 8a1 1 0 1 1 0-2 1 1 0 0 1 0 2Z",
};

function iconHast(type) {
  return {
    type: "element",
    tagName: "svg",
    properties: {
      className: ["admonition-icon"],
      viewBox: "0 0 16 16",
      width: 16,
      height: 16,
      "aria-hidden": "true",
    },
    children: [
      {
        type: "element",
        tagName: "path",
        properties: { d: ICON_PATH[type], fill: "currentColor" },
        children: [],
      },
    ],
  };
}

// Rewrite one blockquote into an admonition when its first line is a marker.
// Returns true when it transformed the node.
function transformBlockquote(node) {
  const first = node.children[0];
  if (!first || first.type !== "paragraph") return false;
  const firstText = first.children[0];
  if (!firstText || firstText.type !== "text") return false;
  const match = MARKER.exec(firstText.value);
  if (!match) return false;
  const type = match[1].toLowerCase();

  // Strip the marker line from the leading text.
  firstText.value = firstText.value.slice(match[0].length);
  // If that emptied the text node (marker was alone on its line), drop it, and
  // drop the paragraph too if it is now empty.
  if (firstText.value === "") first.children.shift();
  if (first.children.length === 0) node.children.shift();

  // The title row: rendered as its own div carrying the octicon and the label.
  const title = {
    type: "paragraph",
    data: {
      hName: "div",
      hProperties: { className: ["admonition-title"] },
      hChildren: [iconHast(type), { type: "text", value: LABEL[type] }],
    },
    children: [],
  };

  node.data = node.data || {};
  node.data.hName = "div";
  node.data.hProperties = { className: ["admonition", `admonition-${type}`] };
  node.children.unshift(title);
  return true;
}

function walk(node) {
  if (!node || !Array.isArray(node.children)) return;
  for (const child of node.children) {
    if (child.type === "blockquote") transformBlockquote(child);
    walk(child);
  }
}

export default function remarkAdmonitions() {
  return (tree) => walk(tree);
}
