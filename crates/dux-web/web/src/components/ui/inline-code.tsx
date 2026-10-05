import { cn } from "@/lib/utils"

// The one chip for a name inside prose: a branch, a path, a file, a command, an
// agent, project or terminal name, a pull request number. The chip is what
// tells the name apart from the sentence around it, so callers drop the quotes
// they used to wrap it in. Sized relative to its surroundings, so it sits in a
// dialog title as comfortably as in body text. It is one inline block, so a
// chip never splits across two rows: one that does not fit the rest of a line
// moves to the next whole, as the terminal UI's name chip does. Capped at the
// line's width, a chip longer than a whole row wraps anywhere inside itself,
// because a path has no spaces to break at and a phone must never scroll
// sideways. It keeps runs of spaces, because in a command they are part of the
// value. To a screen reader it is plain inline text.
function InlineCode({ className, ...props }: React.ComponentProps<"code">) {
  return (
    <code
      data-slot="inline-code"
      className={cn(
        "inline-block max-w-full rounded bg-muted px-1.5 py-0.5 font-mono text-[0.85em] box-decoration-clone whitespace-pre-wrap wrap-anywhere",
        className,
      )}
      {...props}
    />
  )
}

export { InlineCode }
