import * as React from "react"
import { Slot } from "radix-ui"
import { cva, type VariantProps } from "class-variance-authority"

import { cn } from "@/lib/utils"

// Dissent buttons: 40 px by default, 36 px for "sm" (inline), 32 px for "icon". The variants are the ones in
// the old UI's style.css: elevated by default, red for the primary action, danger for destructive ones.
const buttonVariants = cva(
  "inline-flex shrink-0 cursor-pointer items-center justify-center gap-s-2 whitespace-nowrap rounded-lg border border-transparent text-sm font-semibold transition-colors outline-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        default: "bg-primary text-primary-foreground hover:bg-accent-hover",
        destructive: "bg-destructive text-white hover:bg-danger-hover",
        secondary: "bg-elevated text-text-strong hover:bg-elevated-hover",
        outline: "border-border-strong bg-transparent font-medium text-text-strong hover:bg-hover",
        ghost: "bg-transparent font-medium text-muted-foreground hover:bg-hover hover:text-text",
        link: "text-link underline-offset-4 hover:text-link-hover hover:underline",
      },
      size: {
        default: "h-10 px-s-5",
        sm: "h-9 px-3.5 font-medium",
        icon: "size-8 rounded-sm p-0 text-muted-foreground hover:text-text",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  }
)

function Button({
  className,
  variant = "default",
  size = "default",
  asChild = false,
  ...props
}: React.ComponentProps<"button"> &
  VariantProps<typeof buttonVariants> & {
    asChild?: boolean
  }) {
  const Comp = asChild ? Slot.Root : "button"

  return (
    <Comp
      data-slot="button"
      data-variant={variant}
      data-size={size}
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
    />
  )
}

export { Button, buttonVariants }
