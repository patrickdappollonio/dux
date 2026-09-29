import { useSettlingAnimation } from "@/hooks/use-settling-animation"

// The glyph's working cue, shared by every surface that bounces a working
// glyph (the sidebar list's agent and terminal rows, the collapsed rail and the
// agent tab strip): the pulse plus four bounces per pulse on the shared clock
// (`--animate-working-cue`, one `animation` value, so both start on the same
// frame). The bounce is a user-locked decision (CLAUDE.md, "Locked by the
// user").
//
// A stop is the glyph's alone: the state word beside it switches at once,
// because the word is always the truth, while the glyph finishes the bounce in
// flight and rests exactly at translateY(0); its opacity eases back to full
// through the transition in WORKING_GLYPH_CLASS.
//
// `cancel` is a higher state (typing, needs-you, a stopped session) taking the
// glyph over: that is not a stop, and the pulse must never run inside the
// attention blink, so the cue drops there and the transitions ease it back.
export const WORKING_CUE_SETTLE_ON = "working-bounce"
export const WORKING_GLYPH_CLASS =
  "motion-safe:transition-[opacity,transform] motion-safe:duration-300"
export const WORKING_GLYPH_ANIMATION = "motion-safe:animate-working-cue"

export function useWorkingCue<T extends Element>(working: boolean, cancel = false) {
  return useSettlingAnimation<T>(working, WORKING_CUE_SETTLE_ON, { cancel })
}
