import { useEffect, useState } from "react"

// A Blob object URL over `content`, revoked automatically: on content change
// (the previous URL is revoked as the new one is minted), on content going
// null, and on unmount. Object URLs are manual-lifetime: every create must
// be paired with a revoke or the blob leaks for the life of the page, so the
// pairing lives in one effect here rather than at call sites.
//
// Used by the editor's SVG preview: the URL is rebuilt from the CURRENT DRAFT
// on every draft change, which is what keeps the preview draft-accurate like
// markdown's.
export function useObjectUrl(
  content: string | null,
  type: string,
): string | null {
  const [url, setUrl] = useState<string | null>(null)
  useEffect(() => {
    // The synchronous setState below is the deliberate synchronize-with-props
    // shape (mirrors EditorOverlay's loadFileBuffer seed): the URL exists
    // exactly as long as the content it was minted for, and the cleanup is
    // the revoke. React bails out of the re-render when the value is
    // unchanged (null -> null).
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
