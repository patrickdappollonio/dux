// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { Radio } from "@base-ui/react/radio"
import { RadioGroup } from "@base-ui/react/radio-group"

import { Input } from "./input"
import { Switch } from "./switch"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "./select"
import { Textarea } from "./textarea"
import { SETTINGS } from "@/lib/settingsDescriptors"

let phone = false
vi.mock("@/hooks/use-mobile", () => ({
  MOBILE_BREAKPOINT: 768,
  isMobileViewport: () => phone,
  useIsMobile: () => phone,
}))

const { FormGroup, FormSection } = await import("./form-group")
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
          notes={[{ words: "Shown on agents." }]}
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
      <FormGroup layout="stacked" control="single" label="Text" help="Help words." notes={[{ words: "Note words." }]}>
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
      <FormGroup layout="side" control="single" label="Port" help="Help words." notes={[{ words: "Note words." }]}>
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

  it("says so when a field has no name: an empty label, an empty spoken name, a label that draws nothing", () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {})
    const empty = [
      <FormGroup key="a" layout="stacked" control="single" label="">
        {(field) => <Input {...field} />}
      </FormGroup>,
      <FormGroup key="b" layout="stacked" control="single" spokenName="  ">
        {(field) => <Input {...field} />}
      </FormGroup>,
      <FormGroup key="c" layout="stacked" control="single" label={<></>}>
        {(field) => <Input {...field} />}
      </FormGroup>,
    ]
    for (const group of empty) {
      error.mockClear()
      const { unmount } = render(group)
      expect(error, String(group.key)).toHaveBeenCalledWith(expect.stringContaining("has no name"))
      unmount()
    }
  })

  it("raises nothing when the control carries all of its wiring, a select and a group included", () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {})
    render(
      <>
        <FormGroup layout="stacked" control="single" label="Name" help="Help." notes={[{ words: "Note." }]} error="Bad." invalid>
          {(field) => <Input {...field} />}
        </FormGroup>
        <FormGroup layout="stacked" control="single" label="Provider" help="Help." invalid>
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
        </FormGroup>
        <FormGroup layout="stacked" control="group" label="Shown on" help="Help." invalid>
          {(field) => (
            <RadioGroup
              aria-labelledby={field["aria-labelledby"]}
              aria-describedby={field["aria-describedby"]}
              aria-invalid={field["aria-invalid"]}
            >
              <Radio.Root value="a">A</Radio.Root>
            </RadioGroup>
          )}
        </FormGroup>
      </>,
    )
    expect(error).not.toHaveBeenCalled()
  })

  it("says so when the control drops or overrides part of its wiring, not only its id", () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {})
    const cases: [string, React.ReactElement][] = [
      [
        "aria-describedby",
        <FormGroup key="a" layout="stacked" control="single" label="Text" help="Help.">
          {(field) => <Textarea {...field} aria-describedby={undefined} />}
        </FormGroup>,
      ],
      [
        "aria-invalid",
        <FormGroup key="b" layout="stacked" control="single" label="Text" invalid>
          {(field) => <Textarea {...field} aria-invalid={undefined} />}
        </FormGroup>,
      ],
      [
        "aria-invalid",
        <FormGroup key="c" layout="stacked" control="group" label="Shown on" invalid>
          {(field) => (
            <RadioGroup aria-labelledby={field["aria-labelledby"]} aria-describedby={field["aria-describedby"]}>
              <Radio.Root value="a">A</Radio.Root>
            </RadioGroup>
          )}
        </FormGroup>,
      ],
    ]
    for (const [part, group] of cases) {
      error.mockClear()
      const { unmount } = render(group)
      expect(error, String(group.key)).toHaveBeenCalledWith(expect.stringContaining(part))
      unmount()
    }
  })

  it("draws every note line in order, a warning in the warning colour, and reads them all with the field", () => {
    render(
      <FormGroup
        layout="side"
        control="single"
        label="Port"
        help="Help."
        notes={[{ words: "Zero means off." }, { words: "Not saved yet.", tone: "warning" }, { words: undefined }]}
      >
        {(field) => <Input {...field} />}
      </FormGroup>,
    )
    expect(descriptions(screen.getByRole("textbox", { name: "Port" }))).toEqual([
      "Help.",
      "Zero means off.",
      "Not saved yet.",
    ])
    expect(screen.getByText("Zero means off.").className).toContain("text-muted-foreground")
    expect(screen.getByText("Not saved yet.").className).toContain("text-dux-warning")
  })

  it("puts the notes among a side field's words, above its control on a phone, and under a stacked field's control", () => {
    phone = true
    render(
      <>
        <FormGroup layout="side" width="wide" control="single" label="Side" notes={[{ words: "Side note." }]} error="Side error.">
          {(field) => <Input {...field} />}
        </FormGroup>
        <FormGroup layout="stacked" control="single" label="Stack" notes={[{ words: "Stack note." }]} error="Stack error.">
          {(field) => <Input {...field} />}
        </FormGroup>
      </>,
    )
    const follows = (a: Node, b: Node) => Boolean(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING)
    const side = screen.getByRole("textbox", { name: "Side" })
    expect(follows(screen.getByText("Side note."), side)).toBe(true)
    expect(follows(screen.getByText("Side error."), side)).toBe(true)
    const stack = screen.getByRole("textbox", { name: "Stack" })
    expect(follows(stack, screen.getByText("Stack note."))).toBe(true)
    expect(follows(stack, screen.getByText("Stack error."))).toBe(true)
  })

  it("names a control by its words alone, and a press on the words leaves it alone", () => {
    render(
      <FormGroup layout="side" control="named" label="Copy on select" help="Help.">
        {(field) => <Switch {...field} defaultChecked />}
      </FormGroup>,
    )
    const toggle = screen.getByRole("switch", { name: "Copy on select" })
    expect(descriptions(toggle)).toEqual(["Help."])
    fireEvent.click(screen.getByText("Copy on select"))
    expect(toggle.getAttribute("aria-checked")).toBe("true")
  })

  it("says so when a control named by its words alone is a text box, which a press on its label must reach", () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {})
    const boxes: React.ReactElement[] = [
      <FormGroup key="input" layout="side" control="named" label="Size">
        {(field) => <input type="number" {...(field as Record<string, unknown>)} />}
      </FormGroup>,
      <FormGroup key="textarea" layout="side" control="named" label="Text">
        {(field) => <textarea {...(field as Record<string, unknown>)} />}
      </FormGroup>,
    ]
    for (const group of boxes) {
      error.mockClear()
      const { unmount } = render(group)
      expect(error, String(group.key)).toHaveBeenCalledWith(expect.stringContaining("text box"))
      unmount()
    }
  })

  it("draws a setting's config key in monospace, its searched words lit, and reads it with the field", () => {
    const setting = SETTINGS.find((d) => d.key === "ui.copy_on_select")!
    render(
      <FormGroup layout="side" control="single" label="Port" notes={[{ setting, lit: ["copy"] }]}>
        {(field) => <Input {...field} />}
      </FormGroup>,
    )
    expect(descriptions(screen.getByRole("textbox", { name: "Port" }))).toEqual(["ui.copy_on_select"])
    expect(screen.getByText("copy").tagName).toBe("MARK")
    expect(screen.getByText("copy").closest("[id]")!.className).toContain("font-mono")
  })

  it("draws a field with no visible label with its help under the control, where a label would not sit above it", () => {
    render(
      <FormGroup layout="stacked" control="single" spokenName="File name" help="Help words." error="Bad.">
        {(field) => <Input {...field} />}
      </FormGroup>,
    )
    const box = screen.getByRole("textbox", { name: "File name" })
    const follows = (a: Node, b: Node) => Boolean(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING)
    expect(follows(box, screen.getByText("Help words."))).toBe(true)
    expect(follows(screen.getByText("Help words."), screen.getByText("Bad."))).toBe(true)
    expect(descriptions(box)).toEqual(["Help words.", "Bad."])
  })

  it("says a field may be left empty in its label, which the field's name carries", () => {
    render(
      <FormGroup layout="stacked" control="single" label="Project name" optional>
        {(field) => <Input {...field} />}
      </FormGroup>,
    )
    expect(screen.getByRole("textbox", { name: "Project name (optional)" })).toBeTruthy()
  })

  it("sets a compact stacked field's label closer to its control than a regular one's", () => {
    render(
      <>
        <FormGroup layout="stacked" density="compact" control="single" label="Close">
          {(field) => <Input {...field} />}
        </FormGroup>
        <FormGroup layout="stacked" control="single" label="Regular">
          {(field) => <Input {...field} />}
        </FormGroup>
      </>,
    )
    const gap = (name: string) => screen.getByRole("textbox", { name }).closest("[data-form-layout]")!.className
    expect(gap("Close")).toContain("gap-1")
    expect(gap("Regular")).toContain("gap-2")
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

describe("FormSection", () => {
  it("names its fields' group by its title and reads its words, then its error, with the group", () => {
    render(
      <FormSection title="Password" help="Help." notes={[{ words: "Note." }]} error="Refused." errorAnnounce="alert">
        <FormGroup layout="stacked" density="compact" control="single" label="New password">
          {(field) => <Input {...field} />}
        </FormGroup>
      </FormSection>,
    )
    const group = screen.getByRole("group", { name: "Password" })
    expect(descriptions(group)).toEqual(["Help.", "Note.", "Refused."])
    expect(group.contains(screen.getByRole("textbox", { name: "New password" }))).toBe(true)
    expect(group.contains(screen.getByRole("alert"))).toBe(true)
  })

  it("draws its words alone, with no group, when it has no fields", () => {
    render(<FormSection title="Password" help="Set from the command line." />)
    expect(screen.queryByRole("group")).toBeNull()
    expect(screen.getByText("Set from the command line.")).toBeTruthy()
  })

  it("says so when its title is empty, or its group holds no field", () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {})
    const cases: [string, React.ReactElement][] = [
      ["has no title", <FormSection key="title" title=" " help="Help." />],
      ["has no title", <FormSection key="element" title={<></>} help="Help." />],
      [
        "holds no field",
        <FormSection key="empty" title="Password">
          <p>Words, no field.</p>
        </FormSection>,
      ],
    ]
    for (const [part, section] of cases) {
      error.mockClear()
      const { unmount } = render(section)
      expect(error, String(section.key)).toHaveBeenCalledWith(expect.stringContaining(part))
      unmount()
    }
  })
})
