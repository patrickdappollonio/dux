// What a caller of `HeaderButton` cannot write, checked by `tsc -b` in the
// build: each line under a directive is a compile error, and a directive with
// no error under it is one. Nothing imports this file, so none of it reaches
// the bundle.
import { Plus } from "lucide-react"

import { HeaderButton } from "./HeaderButton"

export function HeaderButtonTypeContract({ word }: { word: string | undefined }) {
  return (
    <>
      <HeaderButton icon={<Plus />} label="Word" />
      <HeaderButton icon={<Plus />} aria-label="Named" />
      <HeaderButton icon={<Plus />} label={word} aria-label="Named when it has no word" />
      <HeaderButton icon={<Plus />} label="Word" words="computer" hide="phone" tone="warning" />
      {/* @ts-expect-error an icon alone has no name for a screen reader */}
      <HeaderButton icon={<Plus />} />
      {/* @ts-expect-error a word that may be missing leaves the button nameless */}
      <HeaderButton icon={<Plus />} label={word} />
      {/* @ts-expect-error a caller-chosen class */}
      <HeaderButton icon={<Plus />} label="Word" className="h-20" />
      {/* @ts-expect-error a caller-chosen style */}
      <HeaderButton icon={<Plus />} label="Word" style={{ width: 80 }} />
      {/* @ts-expect-error a tone is one the component names */}
      <HeaderButton icon={<Plus />} label="Word" tone="danger" />
      {/* @ts-expect-error the places a button is hidden are the component's */}
      <HeaderButton icon={<Plus />} label="Word" hide="computer" />
      {/* @ts-expect-error where the word gives way is one the component names */}
      <HeaderButton icon={<Plus />} label="Word" words="never" />
    </>
  )
}
