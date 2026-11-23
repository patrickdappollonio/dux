import { mergeProps } from "@base-ui/react/merge-props"
import { useRender } from "@base-ui/react/use-render"
import { cva, type VariantProps } from "class-variance-authority"

import { cn } from "@/lib/utils"

const badgeVariants = cva(
  "group/badge inline-flex w-fit shrink-0 items-center justify-center gap-1 overflow-hidden rounded-4xl border border-transparent py-0.5 tabular-nums whitespace-nowrap transition-all focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 has-data-[icon=inline-end]:pr-1.5 has-data-[icon=inline-start]:pl-1.5 aria-invalid:border-destructive aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 [&>svg]:pointer-events-none [&>svg]:size-3!",
  {
    variants: {
      variant: {
        default: "bg-primary text-primary-foreground [a]:hover:bg-primary/80",
        secondary:
          "bg-secondary text-secondary-foreground [a]:hover:bg-secondary/80",
        destructive:
          "bg-destructive/10 text-destructive focus-visible:ring-destructive/20 dark:bg-destructive/20 dark:focus-visible:ring-destructive/40 [a]:hover:bg-destructive/20",
        outline:
          "border-border text-foreground [a]:hover:bg-muted [a]:hover:text-muted-foreground",
        ghost:
          "hover:bg-muted hover:text-muted-foreground dark:hover:bg-muted/50",
        link: "text-primary underline-offset-4 hover:underline",
        // A label that says something about a row without competing with its
        // title: a count, "Attached", a provider name.
        muted: "bg-muted text-muted-foreground",
        // A name on a muted fill with a hairline edge: the target a row acts on.
        chip: "border-border bg-muted text-foreground",
        // "This process" in the Task Manager: cyan, and deliberately static,
        // because the blink paired with that colour elsewhere means "needs you".
        self: "border-cyan-100/30 bg-cyan-100/10 text-cyan-100",
      },
      // Every pill is one of two heights. `sm` is the dense one for counts and
      // tags inside rows; `md` is the ordinary one.
      size: {
        md: "h-5 px-2 text-xs font-medium",
        sm: "h-4 px-1.5 text-[11px] leading-none font-normal",
      },
      mono: {
        true: "font-mono",
        false: "",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "md",
      mono: false,
    },
  }
)

function Badge({
  className,
  variant = "default",
  size = "md",
  mono = false,
  render,
  ...props
}: useRender.ComponentProps<"span"> & VariantProps<typeof badgeVariants>) {
  return useRender({
    defaultTagName: "span",
    props: mergeProps<"span">(
      {
        className: cn(badgeVariants({ variant, size, mono }), className),
      },
      props
    ),
    render,
    state: {
      slot: "badge",
      variant,
      size,
    },
  })
}

export { Badge, badgeVariants }
