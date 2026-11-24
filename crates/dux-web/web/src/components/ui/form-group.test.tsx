// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { Radio } from "@base-ui/react/radio"
import { RadioGroup } from "@base-ui/react/radio-group"

import { Input } from "./input"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "./select"
import { Textarea } from "./textarea"

let phone = false
vi.mock("@/hooks/use-mobile", () => ({
  MOBILE_BREAKPOINT: 768,
  isMobileViewport: () => phone,
  useIsMobile: () => phone,
}))

const { FormGroup } = await import("./form-group")
const { CheckboxRow } = await import("@/components/CheckboxRow")

afterEach(() => {
  cleanup()
  phone = false
  vi.restoreAllMocks()
})

// The ids of what describes `el`, in order, read as the words they point at.
function descriptions(el: HTMLElement): string[] {
  return (el.getAttribute("aria-describedby") ?? "")
    .split(/\s+/)
    .filter(Boolean)
    .map((id) => document.getElementById(id)?.textContent ?? `<missing ${id}>`)
}

function Choice({ field }: { field: Record<string, unknown> }) {
  return (
    <RadioGroup
      aria-label={field["aria-label"] as string | undefined}
      aria-labelledby={field["aria-labelledby"] as string | undefined}
      aria-describedby={field["aria-describedby"] as string | undefined}
      defaultValue="b"
    >
      <Radio.Root value="a">A</Radio.Root>
      <Radio.Root value="b">B</Radio.Root>
    </RadioGroup>
  )
}

describe("FormGroup", () => {
  // jsdom runs a label's activation (a click on its control) but not the focus
  // a browser gives a text field then, so the label's control is what is pinned.
  it("ties its label to a single text field, so a click on the label lands in it", () => {
    const clicked = vi.fn()
    render(
      <FormGroup layout="stacked" control="single" label="Name">
        {(field) => <Input {...field} onClick={clicked} />}
      </FormGroup>,
    )
    const box = screen.getByRole("textbox", { name: "Name" })
    const label = screen.getByText("Name") as HTMLLabelElement
    expect(label.control).toBe(box)
    fireEvent.click(label)
    expect(clicked).toHaveBeenCalled()
  })

  it("ties its label to a select's trigger, and a click on the label opens it", async () => {
    render(
      <FormGroup layout="stacked" control="single" label="Provider">
        {(field) => (
          <Select defaultValue="a">
            <SelectTrigger {...field}>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="a">alpha</SelectItem>
            </SelectContent>
          </Select>
        )}
      </FormGroup>,
    )
    const trigger = screen.getByRole("combobox", { name: "Provider" })
    fireEvent.click(screen.getByText("Provider"))
    expect(trigger.getAttribute("aria-expanded")).toBe("true")
  })

  it("names a group of choices as a group, and a click on the label moves focus to its chosen one", () => {
    render(
      <FormGroup layout="stacked" control="group" label="Shown on">
        {(field) => <Choice field={field} />}
      </FormGroup>,
    )
    const group = screen.getByRole("radiogroup", { name: "Shown on" })
    fireEvent.click(screen.getByText("Shown on"))
    expect(document.activeElement).toBe(screen.getByRole("radio", { name: "B" }))
    expect(group.contains(document.activeElement)).toBe(true)
  })

  it("reads the help, then the note, then the error, then lines drawn elsewhere, with the field", () => {
    render(
      <>
        <p id="elsewhere">Two macros share a name.</p>
        <FormGroup
          layout="stacked"
          control="single"
          label="Text"
          help="Each line is sent as Alt+Enter."
          note="Shown on agents."
          error="The text is empty."
          describedBy="elsewhere"
        >
          {(field) => <Textarea {...field} />}
        </FormGroup>
      </>,
    )
    const box = screen.getByRole("textbox", { name: "Text" })
    expect(descriptions(box)).toEqual([
      "Each line is sent as Alt+Enter.",
      "Shown on agents.",
      "The text is empty.",
      "Two macros share a name.",
    ])
    expect(box.getAttribute("aria-invalid")).toBe("true")
  })

  it("says an error tied to its field silently by default, and as an alert when asked", () => {
    const { rerender } = render(
      <FormGroup layout="stacked" control="single" label="Name" error="Taken.">
        {(field) => <Input {...field} />}
      </FormGroup>,
    )
    expect(screen.queryByRole("alert")).toBeNull()
    rerender(
      <FormGroup layout="stacked" control="single" label="Name" error="Taken." errorAnnounce="alert">
        {(field) => <Input {...field} />}
      </FormGroup>,
    )
    expect(screen.getByRole("alert").textContent).toBe("Taken.")
  })

  it("marks the field invalid when a line drawn elsewhere says why", () => {
    render(
      <FormGroup layout="stacked" control="single" label="Name" invalid>
        {(field) => <Input {...field} />}
      </FormGroup>,
    )
    expect(screen.getByRole("textbox", { name: "Name" }).getAttribute("aria-invalid")).toBe("true")
  })

  it("draws the help between the label and the field when stacked", () => {
    render(
      <FormGroup layout="stacked" control="single" label="Text" help="Help words." note="Note words.">
        {(field) => <Textarea {...field} />}
      </FormGroup>,
    )
    const box = screen.getByRole("textbox", { name: "Text" })
    const before = (a: Node, b: Node) => Boolean(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING)
    expect(before(screen.getByText("Text"), screen.getByText("Help words."))).toBe(true)
    expect(before(screen.getByText("Help words."), box)).toBe(true)
    expect(before(box, screen.getByText("Note words."))).toBe(true)
  })

  it("names a field with no visible label by its spoken name alone", () => {
    render(
      <FormGroup layout="stacked" control="single" spokenName="Folder name" help="Inside the folder shown.">
        {(field) => <Input {...field} />}
      </FormGroup>,
    )
    const box = screen.getByRole("textbox", { name: "Folder name" })
    expect(screen.queryByText("Folder name")).toBeNull()
    expect(descriptions(box)).toEqual(["Inside the folder shown."])
  })

  it("names a group with no visible label by its spoken name alone", () => {
    render(
      <FormGroup layout="stacked" control="group" spokenName="Shown on">
        {(field) => <Choice field={field} />}
      </FormGroup>,
    )
    expect(screen.getByRole("radiogroup", { name: "Shown on" })).toBeTruthy()
  })

  it("says a changed mark in words, ahead of the label", () => {
    render(
      <FormGroup layout="side" control="single" label="Theme" changed="Changed from default:">
        {(field) => <Input {...field} />}
      </FormGroup>,
    )
    expect(screen.getByText("Changed from default:")).toBeTruthy()
    expect(screen.getByRole("textbox", { name: "Theme" })).toBeTruthy()
  })

  it("puts the control beside the words on a computer, its note among the words", () => {
    render(
      <FormGroup layout="side" control="single" label="Port" help="Help words." note="Note words.">
        {(field) => <Input {...field} />}
      </FormGroup>,
    )
    const box = screen.getByRole("textbox", { name: "Port" })
    expect(box.closest("[data-form-layout]")?.getAttribute("data-form-layout")).toBe("side")
    expect(Boolean(screen.getByText("Note words.").compareDocumentPosition(box) & Node.DOCUMENT_POSITION_FOLLOWING)).toBe(true)
  })

  it("stacks a wide side field on a phone, through the app's phone answer, and keeps a compact one beside", () => {
    phone = true
    render(
      <>
        <FormGroup layout="side" width="wide" control="single" label="Wide">
          {(field) => <Input {...field} />}
        </FormGroup>
        <FormGroup layout="side" width="compact" control="single" label="Compact">
          {(field) => <Input {...field} />}
        </FormGroup>
      </>,
    )
    const layoutOf = (name: string) =>
      screen.getByRole("textbox", { name }).closest("[data-form-layout]")?.getAttribute("data-form-layout")
    expect(layoutOf("Wide")).toBe("stacked")
    expect(layoutOf("Compact")).toBe("side")
  })

  it("keeps a wide side field beside its words on a computer", () => {
    render(
      <FormGroup layout="side" width="wide" control="single" label="Wide">
        {(field) => <Input {...field} />}
      </FormGroup>,
    )
    expect(
      screen.getByRole("textbox", { name: "Wide" }).closest("[data-form-layout]")?.getAttribute("data-form-layout"),
    ).toBe("side")
  })

  it("draws controls that act on the field on its row, outside its name", () => {
    render(
      <FormGroup layout="stacked" control="single" label="Name" after={<button type="button">More</button>}>
        {(field) => <Input {...field} />}
      </FormGroup>,
    )
    expect(screen.getByRole("textbox", { name: "Name" })).toBeTruthy()
    expect(screen.getByRole("button", { name: "More" })).toBeTruthy()
  })

  it("says so when a caller leaves the wiring off its control", () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {})
    render(
      <FormGroup layout="stacked" control="single" label="Name">
        {() => <Input />}
      </FormGroup>,
    )
    expect(error).toHaveBeenCalledWith(expect.stringContaining("FormGroup"))
  })
})

describe("CheckboxRow", () => {
  it("names the box by its words, ticks it from them, and reads its help with it", () => {
    const onChange = vi.fn()
    render(
      <CheckboxRow id="opt" checked={false} onCheckedChange={onChange} help="Only for this project.">
        Also delete the branch
      </CheckboxRow>,
    )
    const box = screen.getByRole("checkbox", { name: "Also delete the branch" })
    expect(descriptions(box)).toEqual(["Only for this project."])
    fireEvent.click(screen.getByText("Also delete the branch"))
    expect(onChange).toHaveBeenCalledWith(true, expect.anything())
  })
})
