import type { MouseEvent } from "react"
import Markdown, { defaultUrlTransform } from "react-markdown"
import rehypeRaw from "rehype-raw"
import rehypeSanitize from "rehype-sanitize"
import remarkGfm from "remark-gfm"
import { markdownAssetUrl } from "@/lib/markdown"
import { formatFrontMatterValue, splitFrontMatter } from "@/lib/frontMatter"
import type { EditorRoot } from "@/lib/editorRoot"

interface MarkdownPreviewProps {
  // The current editor buffer (so the preview reflects unsaved edits).
  content: string
  // The session whose worktree backs the relative-image proxy.
  root: EditorRoot
  // The markdown file's worktree path: relative image `src`s resolve against its
  // directory. Null when no file is open (relative images then aren't rewritten).
  path: string | null
}

// Rendered markdown for the editor's preview toggle, lazy-loaded and styled from
// theme tokens. Previewed markdown is NOT author-trusted and the preview runs in
// dux's own origin, so embedded HTML is rendered through rehype-raw and then
// sanitized on GitHub's schema: sanitize must run AFTER raw.
export default function MarkdownPreview({
  content,
  root,
  path,
}: MarkdownPreviewProps) {
  // Rewrite a relative image `src` to the worktree asset proxy, so it resolves
  // against the markdown file's directory rather than the SPA's URL.
  function transformUrl(url: string, key: string): string {
    if (key === "src" && path !== null) {
      const proxied = markdownAssetUrl(root, path, url)
      if (proxied !== null) return proxied
    }
    return defaultUrlTransform(url)
  }

  // Open links in a new tab by delegation rather than a custom `a` renderer: a
  // click in the preview must never navigate the SPA away.
  function onLinkClick(e: MouseEvent<HTMLDivElement>): void {
    const anchor = (e.target as HTMLElement).closest("a")
    if (!anchor) return
    // In-page and href-less anchors stay inert: opening one would spawn a bogus
    // SPA tab, and react-markdown adds no heading ids to scroll to anyway.
    const href = anchor.getAttribute("href")
    if (!href || href.startsWith("#")) return
    e.preventDefault()
    window.open(anchor.href, "_blank", "noopener,noreferrer")
  }

  // Front matter renders as a table above the prose, its values as React text
  // so they are escaped. `splitFrontMatter` owns the extraction rather than
  // remark-frontmatter, which would also strip a SECOND `--- … ---` block.
  const front = splitFrontMatter(content)
  const body = front === null ? content : front.body

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
        {front !== null && front.rows.length > 0 && (
          <table>
            <thead>
              <tr>
                <th scope="col">Key</th>
                <th scope="col">Value</th>
              </tr>
            </thead>
            <tbody>
              {front.rows.map((row, index) => (
                <tr key={`${row.key}-${index}`}>
                  <th scope="row">{row.key}</th>
                  {/* A long unbroken value (a URL, a hash) must wrap rather
                      than push the table past its column. */}
                  <td className="break-words whitespace-pre-wrap">
                    {formatFrontMatterValue(row.value)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        {front !== null && front.unreadable !== null && (
          // The block was stripped from the body, so text this reader produced
          // no rows for is shown verbatim instead of disappearing.
          <pre>
            <code>{front.unreadable}</code>
          </pre>
        )}
        <Markdown
          remarkPlugins={[remarkGfm]}
          rehypePlugins={[rehypeRaw, rehypeSanitize]}
          urlTransform={transformUrl}
        >
          {body}
        </Markdown>
      </div>
    </div>
  )
}
