// `md:` means the computer layout and `max-md:` the phone layout, not a
// width: a landscape phone is wide and still gets the phone layout, and a
// person can choose either layout by hand. lib/shellLayout.ts decides and
// writes `data-layout` on <html>. `:where` adds no specificity, as the media
// queries these replace added none.
//
// This is a plugin rather than two `@custom-variant` lines because of where
// Tailwind sorts them. Redefining a variant keeps its place in the stylesheet,
// but `max-md` is not a variant of its own: it is the functional `max` variant
// with the value `md`, so a `@custom-variant max-md` is a new variant sorted
// after every other one, and would win every tie it should lose (against
// `sm:`, `md:`, `pointer-coarse:`, the `data-*` variants and the rest).
// Redefining `max` itself keeps `max-md` where it always sorted; every other
// `max-*` stays the media query Tailwind builds.
import plugin from "tailwindcss/plugin"

const PHONE_LAYOUT = "&:where(:root[data-layout=\"phone\"] *)"
const COMPUTER_LAYOUT = "&:where(:root[data-layout=\"computer\"] *)"

export default plugin(({ addVariant, matchVariant, theme }) => {
  addVariant("md", COMPUTER_LAYOUT)
  const screens: Record<string, string> = { ...theme("screens") }
  screens.md = PHONE_LAYOUT
  matchVariant(
    "max",
    (value: string) =>
      value === PHONE_LAYOUT ? PHONE_LAYOUT : `@media (width < ${value})`,
    { values: screens },
  )
})
