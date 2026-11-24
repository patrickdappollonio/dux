// What a caller of `EllipsisMenuTrigger` cannot write, checked by `tsc -b` in
// the build: each line under a directive is a compile error, and a directive
// with no error under it is one. Nothing imports this file, so none of it
// reaches the bundle.
import { EllipsisMenuTrigger } from "./EllipsisMenuTrigger"

export function EllipsisTypeContract() {
  return (
    <>
      <EllipsisMenuTrigger placement="row" reveal="hover" fit="slim" aria-label="Row" />
      <EllipsisMenuTrigger placement="header" phoneOnly aria-label="Header" />
      <EllipsisMenuTrigger placement="tab-pill" reveal="hover-or-focus" aria-label="Pill" />
      {/* @ts-expect-error a pill's box is its own, roomy or not */}
      <EllipsisMenuTrigger placement="tab-pill" fit="roomy" aria-label="Pill" />
      {/* @ts-expect-error a pill's box is its own, slim or not */}
      <EllipsisMenuTrigger placement="tab-pill" fit="slim" aria-label="Pill" />
      {/* @ts-expect-error a pill is not left out of the computer layout */}
      <EllipsisMenuTrigger placement="tab-pill" phoneOnly aria-label="Pill" />
      {/* @ts-expect-error a row's fits are alternatives, never two at once */}
      <EllipsisMenuTrigger placement="row" fit={["slim", "roomy"]} aria-label="Row" />
      {/* @ts-expect-error a header's box is one, and its neighbours share it; no fit is a header's */}
      <EllipsisMenuTrigger placement="header" fit="roomy" aria-label="Header" />
      {/* @ts-expect-error no fit is a header's, slim and compact being a row's */}
      <EllipsisMenuTrigger placement="header" fit="slim" aria-label="Header" />
      {/* @ts-expect-error a row is never left out of the computer layout */}
      <EllipsisMenuTrigger placement="row" phoneOnly aria-label="Row" />
      {/* @ts-expect-error only a row or a pill sits in a reveal wrapper */}
      <EllipsisMenuTrigger placement="header" reveal="hover" aria-label="Header" />
      {/* @ts-expect-error a toolbar button has no reveal */}
      <EllipsisMenuTrigger placement="toolbar" reveal="hover-or-focus" aria-label="Tools" />
      {/* @ts-expect-error a banner's control has no reveal */}
      <EllipsisMenuTrigger placement="banner" reveal="hover" aria-label="Banner" />
      {/* @ts-expect-error the floating pill's box never changes */}
      <EllipsisMenuTrigger placement="floating" fit="roomy" aria-label="Floating" />
      {/* @ts-expect-error a caller-chosen class */}
      <EllipsisMenuTrigger placement="row" className="size-20" aria-label="Row" />
      {/* @ts-expect-error a caller-chosen style */}
      <EllipsisMenuTrigger placement="row" style={{ width: 80 }} aria-label="Row" />
    </>
  )
}
