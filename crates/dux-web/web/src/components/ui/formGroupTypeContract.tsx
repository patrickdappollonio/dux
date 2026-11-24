// What a caller of `FormGroup` cannot write, checked by `tsc -b` in the build:
// each line under a directive is a compile error, and a directive with no error
// under it is one. Nothing imports this file, so none of it reaches the bundle.
import { FormGroup } from "./form-group"

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
        {(field) => <input {...field} />}
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
