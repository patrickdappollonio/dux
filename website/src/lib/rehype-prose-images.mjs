// Markdown image enhancements for prose pages (docs + blog). Runs as a rehype
// plugin on the shared markdown processor (see astro.config.mjs), so plain
// Markdown images get two upgrades without any HTML in the post:
//
//   1. Alignment via URL hash. `![alt](/img.png#right)` drops the `#right` from
//      the src and adds an `align-right` class. Supported: left, right, center,
//      full (full-width). No hash → default block image.
//
//   2. Modern formats. A local raster image (png/jpg/jpeg) is wrapped in a
//      <picture> that offers a .webp <source> with the original as the <img>
//      fallback, so the browser picks the smaller format. SVGs (already
//      vector) and remote images are left as a plain <img>. The .webp siblings
//      are produced at build time by scripts/generate-webp.mjs, which scans the
//      same public rasters — so a <source> this plugin emits always resolves.
//
// Written as a small manual hast walk to avoid a unist-util-visit dependency.

const ALIGN_CLASS = {
  left: "align-left",
  right: "align-right",
  center: "align-center",
  full: "full-width",
};

// A local raster path we can offer a webp sibling for (served from /public).
const LOCAL_RASTER = /^\/.+\.(png|jpe?g)$/i;

const toWebp = (src) => src.replace(/\.(png|jpe?g)$/i, ".webp");

function addClass(properties, cls) {
  const existing = properties.className;
  if (Array.isArray(existing)) existing.push(cls);
  else if (typeof existing === "string")
    properties.className = [existing, cls];
  else properties.className = [cls];
}

// Mutates an <img> node (alignment + cleaned src). Returns a replacement node
// (a <picture> wrapper) when the image should be wrapped, otherwise null.
function transformImg(node) {
  const properties = node.properties ?? (node.properties = {});
  let src = typeof properties.src === "string" ? properties.src : "";
  if (!src) return null;

  // 1. Pull a trailing #alignment off the URL.
  const hash = src.indexOf("#");
  if (hash !== -1) {
    const keyword = src.slice(hash + 1).toLowerCase();
    src = src.slice(0, hash);
    properties.src = src;
    if (ALIGN_CLASS[keyword]) addClass(properties, ALIGN_CLASS[keyword]);
  }

  // 2. Wrap a local raster in <picture> with a webp source.
  if (LOCAL_RASTER.test(src)) {
    return {
      type: "element",
      tagName: "picture",
      properties: {},
      children: [
        {
          type: "element",
          tagName: "source",
          properties: { srcset: toWebp(src), type: "image/webp" },
          children: [],
        },
        node,
      ],
    };
  }
  return null;
}

function walk(node) {
  if (!node || !Array.isArray(node.children)) return;
  for (let i = 0; i < node.children.length; i++) {
    const child = node.children[i];
    if (child.type === "element" && child.tagName === "img") {
      const replacement = transformImg(child);
      if (replacement) {
        // Swap the <img> for its <picture> wrapper; the wrapped <img> is
        // already processed, so don't descend into it.
        node.children[i] = replacement;
        continue;
      }
    }
    walk(child);
  }
}

export default function rehypeProseImages() {
  return (tree) => walk(tree);
}
