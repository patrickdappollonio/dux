// What a caller of `FormGroup` cannot write, checked by `tsc -b` in the build:
// each line under a directive is a compile error, and a directive with no error
// under it is one. Nothing imports this file, so none of it reaches the bundle.
import { FormGroup, FormSection } from "./form-group"
import { AgentNameInput } from "@/components/AgentNameFields"
import { Input } from "./input"
import { Switch } from "./switch"
import { Textarea } from "./textarea"
import { SETTINGS } from "@/lib/settingsDescriptors"

export function FormGroupTypeContract({ name }: { name: string | undefined }) {
  const words: string | false | null = name === undefined ? null : name
  return (
    <>
      <FormGroup layout="stacked" control="single" label="Name">
        {(field) => <input {...field} />}
      </FormGroup>
      <FormGroup layout="stacked" control="single" spokenName="Name">
        {(field) => <input {...field} />}
      </FormGroup>
      <FormGroup layout="side" width="wide" control="group" label="Shown on" help="Where it shows.">
        {(field) => <div role="radiogroup" {...field} />}
      </FormGroup>
      <FormGroup layout="stacked" height="fill" control="single" label="Text">
        {(field) => <textarea {...field} />}
      </FormGroup>
      <FormGroup layout="side" control="named" label="Copy" notes={[{ setting: SETTINGS[0], lit: ["copy"] }]}>
        {(field) => <button type="button" {...field} />}
      </FormGroup>
      <FormGroup layout="side" control="named" label="Copy">
        {(field) => <Switch {...field} />}
      </FormGroup>
      <FormGroup layout="stacked" control="single" label="Project name" optional>
        {(field) => <Input {...field} />}
      </FormGroup>
      <FormGroup layout="side" control="named" label="Size">
        {/* @ts-expect-error a text box is a single control, so a press on its label reaches it */}
        {(field) => <input type="number" {...field} />}
      </FormGroup>
      <FormGroup layout="side" control="named" label="Size">
        {/* @ts-expect-error a text box is a single control, so a press on its label reaches it */}
        {(field) => <Input {...field} />}
      </FormGroup>
      <FormGroup layout="side" control="named" label="Text">
        {/* @ts-expect-error a text area is a single control, so a press on its label reaches it */}
        {(field) => <Textarea {...field} />}
      </FormGroup>
      <FormGroup layout="stacked" control="single" spokenName="Agent name">
        {(field) => <AgentNameInput field={field} value="" onChange={() => {}} onSubmit={() => {}} generating={false} autoFocus={false} />}
      </FormGroup>
      <FormGroup layout="side" control="named" label="Name">
        {/* @ts-expect-error a wrapper around a text box refuses a named control's wiring */}
        {(field) => <AgentNameInput field={field} value="" onChange={() => {}} onSubmit={() => {}} generating={false} autoFocus={false} />}
      </FormGroup>
      {/* @ts-expect-error a field with no visible label has no "(optional)" to draw */}
      <FormGroup layout="stacked" control="single" spokenName="Name" optional>
        {(field) => <input {...field} />}
      </FormGroup>
      <FormGroup layout="stacked" control="single" spokenName="Path" help="Help." helpAt="above">
        {(field) => <input {...field} />}
      </FormGroup>
      {/* @ts-expect-error a labelled field's help already sits above its control */}
      <FormGroup layout="stacked" control="single" label="Path" help="Help." helpAt="above">
        {(field) => <input {...field} />}
      </FormGroup>
      {/* @ts-expect-error where help sits is a stacked field's choice */}
      <FormGroup layout="side" control="single" spokenName="Path" help="Help." helpAt="above">
        {(field) => <input {...field} />}
      </FormGroup>
      <FormGroup layout="stacked" density="compact" control="single" label="New password">
        {(field) => <input {...field} />}
      </FormGroup>
      <FormSection title="Password" help="Help." notes={[{ words: "Note." }]}>
        <input />
      </FormSection>
      {/* @ts-expect-error a density is a stacked field's choice */}
      <FormGroup layout="side" density="compact" control="single" label="Name">
        {(field) => <input {...field} />}
      </FormGroup>
      {/* @ts-expect-error a config key's note has no tone of its own */}
      <FormGroup layout="side" control="single" label="Name" notes={[{ setting: SETTINGS[0], tone: "warning" }]}>
        {(field) => <input {...field} />}
      </FormGroup>
      {/* @ts-expect-error only a setting's key is drawn in monospace, never words that may hold a name */}
      <FormGroup layout="side" control="single" label="Name" notes={[{ words: name, code: true }]}>
        {(field) => <input {...field} />}
      </FormGroup>
      {/* @ts-expect-error a section has a title */}
      <FormSection help="Help." />
      {/* @ts-expect-error a title that may be missing leaves the group nameless */}
      <FormSection title={name} />
      {/* @ts-expect-error a caller-chosen class */}
      <FormSection title="Password" className="gap-1" />
      {/* @ts-expect-error a field with neither a label nor a spoken name has no name */}
      <FormGroup layout="stacked" control="single">
        {(field) => <input {...field} />}
      </FormGroup>
      {/* @ts-expect-error a visible label and a spoken name would be two names */}
      <FormGroup layout="stacked" control="single" label="Name" spokenName="Name">
        {(field) => <input {...field} />}
      </FormGroup>
      {/* @ts-expect-error a spoken name that may be missing leaves the field nameless */}
      <FormGroup layout="stacked" control="single" spokenName={name}>
        {(field) => <input {...field} />}
      </FormGroup>
      {/* @ts-expect-error a field picks its layout */}
      <FormGroup control="single" label="Name">
        {(field) => <input {...field} />}
      </FormGroup>
      {/* @ts-expect-error a label that may be missing leaves the field nameless */}
      <FormGroup layout="stacked" control="single" label={name}>
        {(field) => <input {...field} />}
      </FormGroup>
      {/* @ts-expect-error an absent label is no name */}
      <FormGroup layout="stacked" control="single" label={undefined}>
        {(field) => <input {...field} />}
      </FormGroup>
      {/* @ts-expect-error a label that may be null or false is no name */}
      <FormGroup layout="stacked" control="single" label={words}>
        {(field) => <input {...field} />}
      </FormGroup>
      {/* @ts-expect-error a number is not words */}
      <FormGroup layout="stacked" control="single" label={0}>
        {(field) => <input {...field} />}
      </FormGroup>
      {/* @ts-expect-error a note line is a line of words with a named tone */}
      <FormGroup layout="stacked" control="single" label="Name" notes={[{ words: "x", tone: "red" }]}>
        {(field) => <input {...field} />}
      </FormGroup>
      {/* @ts-expect-error a field says whether its control is one or a group */}
      <FormGroup layout="stacked" label="Name">
        {(field) => <button type="button" {...field} />}
      </FormGroup>
      {/* @ts-expect-error a width is a side field's choice */}
      <FormGroup layout="stacked" width="wide" control="single" label="Name">
        {(field) => <input {...field} />}
      </FormGroup>
      {/* @ts-expect-error a height is a stacked field's choice */}
      <FormGroup layout="side" height="fill" control="single" label="Name">
        {(field) => <input {...field} />}
      </FormGroup>
      {/* @ts-expect-error a caller-chosen class */}
      <FormGroup layout="stacked" control="single" label="Name" className="text-xs">
        {(field) => <input {...field} />}
      </FormGroup>
      {/* @ts-expect-error a caller-chosen style */}
      <FormGroup layout="stacked" control="single" label="Name" style={{ gap: 4 }}>
        {(field) => <input {...field} />}
      </FormGroup>
      <FormGroup layout="stacked" control="single" label="Name">
        {/* @ts-expect-error the control is drawn from the wiring it is handed */}
        <input />
      </FormGroup>
    </>
  )
}
