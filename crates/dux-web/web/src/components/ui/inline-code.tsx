import { cn } from "@/lib/utils"

// The one chip for a name inside prose: a branch, a path, a file, a command, an
// agent, project or terminal name, a pull request number. The chip is what
// tells the name apart from the sentence around it, so callers drop the quotes
// they used to wrap it in. Sized relative to its surroundings, so it sits in a
// dialog title as comfortably as in body text. To a screen reader it is plain
// inline text.
//
// It is an inline box, not an inline block, so a chip that has to wrap flows
// like the words around it: every row it spans gets a background of its own
// that hugs that row's text (`box-decoration-clone`), and the sentence carries
// on right after the chip ends. An inline block that wraps fills the whole line
// instead, which turned a long path in a toast into a full-width slab with the
// rest of the sentence pushed under it.
//
// A chip breaks where the words around it do: at a space or after a hyphen.
// Its spaces are kept (`pre-wrap`), because in a command they are part of the
// value, and nothing is substituted for them, so a copied chip is byte for byte
// the value. Breaking anywhere else is a last resort: a run with no break point
// (a path segment, a file name) that does not fit the rest of a row moves to
// the next one whole and is split mid-run only when it is longer than a row.
// `anywhere` rather than `break-word` because it also shrinks the chip's
// minimum width, so a long path cannot stretch a flex or grid cell, or a
// toast, past the screen.
function InlineCode({ className, ...props }: React.ComponentProps<"code">) {
  return (
    <code
      data-slot="inline-code"
      className={cn(
        "inline max-w-full rounded bg-muted px-1.5 py-0.5 font-mono text-[0.85em] box-decoration-clone whitespace-pre-wrap wrap-anywhere",
        className,
      )}
      {...props}
    />
  )
}

export { InlineCode }
