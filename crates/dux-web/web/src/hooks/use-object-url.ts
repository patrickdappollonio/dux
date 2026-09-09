import { useEffect, useState } from "react"

// A Blob object URL over `content`, revoked automatically on a content change, on content
// going null, and on unmount. Object URLs are manual-lifetime, so every create must be paired
// with a revoke or the blob leaks for the life of the page, and the pairing lives in one effect
// here rather than at call sites. The editor's SVG preview rebuilds it from the current draft.
export function useObjectUrl(
  content: string | null,
  type: string,
): string | null {
  const [url, setUrl] = useState<string | null>(null)
  useEffect(() => {
    // The synchronous setState below is the deliberate synchronize-with-props shape: the URL
    // exists exactly as long as the content it was minted for, and the cleanup is the revoke.
    if (content === null) {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setUrl(null)
      return
    }
    const next = URL.createObjectURL(new Blob([content], { type }))
    setUrl(next)
    return () => URL.revokeObjectURL(next)
  }, [content, type])
  return url
}
