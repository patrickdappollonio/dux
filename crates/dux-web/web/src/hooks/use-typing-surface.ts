import * as React from "react"

import {
  readTypingSurface,
  subscribeTypingSurface,
  type TypingSurface,
} from "@/lib/typingSurface"

/**
 * The device-local typing-surface choice, live: every open pane re-renders when
 * one of them flips the toggle.
 *
 * Same construction as `useIsCoarsePointer` and `useIsMobile`: the value is
 * read during render through `useSyncExternalStore` rather than mirrored into
 * state in an effect, so there is no initial flash and no synchronous setState
 * in an effect. The server snapshot is "unchosen", which lands on the pointer
 * capability, the same place the feature starts on a device nobody has touched
 * the toggle on.
 */
export function useTypingSurface(): TypingSurface | null {
  return React.useSyncExternalStore(
    subscribeTypingSurface,
    readTypingSurface,
    () => null,
  )
}

