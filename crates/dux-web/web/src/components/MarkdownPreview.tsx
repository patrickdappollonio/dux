import type { MouseEvent } from "react"
import Markdown, { defaultUrlTransform } from "react-markdown"
import rehypeRaw from "rehype-raw"
import rehypeSanitize from "rehype-sanitize"
import remarkFrontmatter from "remark-frontmatter"
import remarkGfm from "remark-gfm"
import { markdownAssetUrl } from "@/lib/markdown"

interface MarkdownPreviewProps {
  // The current editor buffer (so the preview reflects unsaved edits).
  content: string
  // The session whose worktree backs the relative-image proxy.
  sessionId: string
  // The markdown file's worktree path — relative image `src`s resolve against its
  // directory. Null when no file is open (relative images then aren't rewritten).
  path: string | null
}

// Rendered markdown for the editor's preview toggle. Lazy-loaded (react-markdown
// is pulled in only when the user previews), styled with theme tokens via
// arbitrary child-element variants so it tracks the app palette without the
// Tailwind typography plugin.
//
// Plugins: remark-gfm (tables, task lists, strikethrough, autolinks);
// remark-frontmatter (recognizes a leading YAML `--- … ---` block so it's omitted
// from the preview instead of rendering as a stray rule + key:value text);
// rehype-raw (renders embedded HTML rather than escaping it); then rehype-sanitize
// (its default = GitHub's schema). Embedded HTML therefore renders the way it does
// on GitHub — common formatting tags (div, details/summary, table, kbd, sub/sup,
// task-list inputs, …) are kept, while <script>, inline event handlers, and
// javascript: URLs are stripped. Previewed markdown is NOT always author-trusted
// (a cloned repo's README, a PR under review, agent-generated output), and a
// preview runs in dux's authenticated origin — so we render "what markdown
// expects" without opening a script-injection hole. sanitize must run AFTER raw.
export default function MarkdownPreview({
  content,
  sessionId,
  path,
}: MarkdownPreviewProps) {
  // Rewrite a relative image `src` to the auth-gated worktree asset proxy so a
  // README's relative images resolve against the markdown file's directory rather
  // than the SPA's URL (where they'd 404). Links and external/absolute URLs fall
  // through to react-markdown's default (safe) URL handling unchanged.
  function transformUrl(url: string, key: string): string {
    if (key === "src" && path !== null) {
      const proxied = markdownAssetUrl(sessionId, path, url)
      if (proxied !== null) return proxied
    }
    return defaultUrlTransform(url)
  }

  // Open links in a new tab via event delegation rather than a custom `a`
  // renderer — a click in the preview must never navigate the SPA away, and this
  // keeps the markdown component free of react-markdown's injected `node` prop.
  function onLinkClick(e: MouseEvent<HTMLDivElement>): void {
    const anchor = (e.target as HTMLElement).closest("a")
    if (!anchor) return
    // Leave in-page anchor links (#section) and href-less anchors inert: opening
    // them in a new tab would just spawn a bogus SPA tab (and react-markdown adds
    // no heading ids, so they wouldn't scroll anyway). Only real links open out.
    const href = anchor.getAttribute("href")
    if (!href || href.startsWith("#")) return
    e.preventDefault()
    window.open(anchor.href, "_blank", "noopener,noreferrer")
  }

  return (
    <div className="h-full overflow-auto" onClick={onLinkClick}>
      <div
        className={[
          "mx-auto max-w-3xl px-6 py-5 text-sm leading-relaxed text-foreground",
          "[&_h1]:mt-6 [&_h1]:mb-3 [&_h1]:text-2xl [&_h1]:font-semibold [&_h1]:tracking-tight",
          "[&_h2]:mt-6 [&_h2]:mb-3 [&_h2]:text-xl [&_h2]:font-semibold",
          "[&_h3]:mt-5 [&_h3]:mb-2 [&_h3]:text-lg [&_h3]:font-semibold",
          "[&_h4]:mt-4 [&_h4]:mb-2 [&_h4]:text-base [&_h4]:font-semibold",
          "[&_p]:my-3",
          "[&_a]:text-primary [&_a]:underline [&_a]:underline-offset-2",
          "[&_strong]:font-semibold",
          "[&_ul]:my-3 [&_ul]:list-disc [&_ul]:pl-6",
          "[&_ol]:my-3 [&_ol]:list-decimal [&_ol]:pl-6",
          "[&_li]:my-1",
          "[&_blockquote]:my-3 [&_blockquote]:border-l-2 [&_blockquote]:border-border [&_blockquote]:pl-4 [&_blockquote]:text-muted-foreground",
          "[&_hr]:my-6 [&_hr]:border-border",
          "[&_code]:rounded [&_code]:bg-muted [&_code]:px-1.5 [&_code]:py-0.5 [&_code]:font-mono [&_code]:text-[0.85em]",
          "[&_pre]:my-4 [&_pre]:overflow-auto [&_pre]:rounded-lg [&_pre]:border [&_pre]:bg-muted [&_pre]:p-3",
          "[&_pre_code]:bg-transparent [&_pre_code]:p-0 [&_pre_code]:text-[0.85em]",
          "[&_table]:my-4 [&_table]:w-full [&_table]:border-collapse [&_table]:text-left",
          "[&_th]:border [&_th]:border-border [&_th]:px-3 [&_th]:py-1.5 [&_th]:font-semibold",
          "[&_td]:border [&_td]:border-border [&_td]:px-3 [&_td]:py-1.5",
          "[&_img]:max-w-full [&_img]:rounded",
        ].join(" ")}
      >
        <Markdown
          remarkPlugins={[remarkFrontmatter, remarkGfm]}
          rehypePlugins={[rehypeRaw, rehypeSanitize]}
          urlTransform={transformUrl}
        >
          {content}
        </Markdown>
      </div>
    </div>
  )
}
