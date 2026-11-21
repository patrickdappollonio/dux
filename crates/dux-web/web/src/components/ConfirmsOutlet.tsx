import { useLayoutEffect, useState } from "react"
import type { ReactNode } from "react"
import type { ConfirmsSetterRef } from "@/hooks/use-publish-confirms"

// Where a dialog body's confirms render: beside the dialog rather than inside
// it, because base-ui draws no overlay for a dialog nested in another, and a
// confirm dims what is behind it as the app's other confirms do. A sibling, so
// the body handing it its confirms on every render re-renders only this.
export function ConfirmsOutlet({ setterRef }: { setterRef: ConfirmsSetterRef }) {
  const [confirms, setConfirms] = useState<ReactNode>(null)
  useLayoutEffect(() => {
    setterRef.current = setConfirms
    return () => {
      setterRef.current = null
    }
  }, [setterRef])
  return confirms
}
