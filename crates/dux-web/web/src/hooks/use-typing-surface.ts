import * as React from "react"

import {
  readTypingSurface,
  subscribeTypingSurface,
  type TypingSurface,
} from "@/lib/typingSurface"
import { composeBarMode, touchSurfacesApply } from "@/lib/composebar"
import { useDux } from "@/lib/store"

import { useIsCoarsePointer } from "./use-coarse-pointer"

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

/**
 * Do the touch typing surfaces (the compose box and the accessory keys) belong
 * on this device at all, once the setting, the pointer's default and the
 * device-local choice are folded together?
 *
 * The header menus off the phone shell read this so their "Show terminal keys"
 * item is present exactly where pressing it puts a key row on screen. They used
 * to ride the width breakpoint alone, which made the item inert in a narrow
 * window on a laptop: the preference flipped and nothing appeared, because the
 * key row was gated on the pointer.
 */
export function useTouchSurfaces(): boolean {
  const coarsePointer = useIsCoarsePointer()
  const choice = useTypingSurface()
  const mode = composeBarMode(useDux().bootstrap?.compose_bar)
  return touchSurfacesApply(mode, coarsePointer, choice)
}
