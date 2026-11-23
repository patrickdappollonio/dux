// What a caller of `EmptyState` cannot write, checked by `tsc -b` in the build:
// each line under a directive is a compile error, and a directive with no error
// under it is one. Nothing imports this file, so none of it reaches the bundle.
import { EmptyState } from "./empty"

export function EmptyStateTypeContract() {
  return (
    <>
      {/* @ts-expect-error a status region would announce the buttons inside it */}
      <EmptyState size="page" icon={<i />} title="x" announce actions={<button />} />
      {/* @ts-expect-error a compact one has no icon */}
      <EmptyState size="compact" title="x" icon={<i />} />
      {/* @ts-expect-error a compact one has no buttons */}
      <EmptyState size="compact" title="x" actions={<button />} />
      {/* @ts-expect-error a roomy one needs its icon */}
      <EmptyState size="section" title="x" />
    </>
  )
}
