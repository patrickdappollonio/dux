// What a caller cannot write, checked by `tsc -b` in the build: each line under
// a directive is a compile error, and a directive with no error under it is one.
// Nothing imports this file, so none of it reaches the bundle.
import { FolderPlus } from "lucide-react"

import { Modal } from "./Modal"
import type { ModalAction } from "./types"

const act: ModalAction = { label: "Go", tone: "primary", onSelect: () => {} }
const common = { open: true, onClose: () => {}, title: "x" }

export function ModalTypeContract() {
  return (
    <>
      {/* @ts-expect-error no kind */}
      <Modal {...common} />
      {/* @ts-expect-error a confirm without its safe button */}
      <Modal kind="confirm" {...common} confirm={act} />
      {/* @ts-expect-error a confirm without its action */}
      <Modal kind="confirm" {...common} cancelLabel="Cancel" />
      {/* @ts-expect-error a form without its submit */}
      <Modal kind="form" {...common} />
      {/* @ts-expect-error a caller-chosen class */}
      <Modal kind="report" {...common} className="max-h-96" />
      {/* @ts-expect-error a caller-chosen style */}
      <Modal kind="report" {...common} style={{ height: 400 }} />
      {/* @ts-expect-error a height that is not a token */}
      <Modal kind="report" {...common} height="h-96" />
      {/* @ts-expect-error a width that is not a token */}
      <Modal kind="report" {...common} size="sm:max-w-[610px]" />
      {/* @ts-expect-error a confirm takes no height: it is a short question */}
      <Modal kind="confirm" {...common} cancelLabel="Cancel" confirm={act} height="h640" />
      {/* @ts-expect-error an action without its tone */}
      <Modal kind="form" {...common} submit={{ label: "Go", onSelect: () => {} }} />
      {/* @ts-expect-error a button action with nothing to run */}
      <Modal kind="form" {...common} submit={{ label: "Go", tone: "primary" }} />
      {/* @ts-expect-error an empty row of doors */}
      <Modal kind="picker" {...common} doors={[]} />
      {/* @ts-expect-error unsaved edits without what discarding leaves as it was */}
      <Modal kind="form" {...common} submit={act} unsavedEdits={{ holder: "It has", changes: 1 }} />
      {/* @ts-expect-error only a form has edits to lose */}
      <Modal kind="report" {...common} unsavedEdits={null} />
      <Modal
        kind="picker"
        {...common}
        doors={[{ label: "Add…", icon: FolderPlus, onSelect: () => {} }]}
      />
    </>
  )
}
