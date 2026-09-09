import * as React from "react"

import {
  readTypingSurface,
  subscribeTypingSurface,
  type TypingSurface,
} from "@/lib/typingSurface"

/**
 * The device-local typing-surface choice, live: every open pane re-renders when
 * one of them flips the toggle. Read during render through
 * `useSyncExternalStore`, so there is no initial flash and no synchronous
 * setState in an effect; the server snapshot is "unchosen", which lands on the
 * pointer capability, where a device nobody has touched the toggle on starts.
 */
export function useTypingSurface(): TypingSurface | null {
  return React.useSyncExternalStore(
    subscribeTypingSurface,
    readTypingSurface,
    () => null,
  )
}

