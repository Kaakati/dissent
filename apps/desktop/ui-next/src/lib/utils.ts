import { clsx, type ClassValue } from "clsx"
import { extendTailwindMerge } from "tailwind-merge"

// Dissent's spacing scale is named s-1, s-2, … (see @theme in index.css). Teach tailwind-merge those names, so a
// className like "px-s-3" replaces a component's default "px-s-5" instead of both being kept.
// The type sizes are named text-body, text-meta and so on (see @theme in index.css). Without this, tailwind-merge reads
// "text-meta" as a colour and drops it when a later "text-muted-foreground" is merged in, so the text takes its parent's size.
const twMerge = extendTailwindMerge({
  extend: {
    theme: {
      spacing: ["s-1","s-2","s-3","s-4","s-5","s-6","s-8","s-10","s-12"],
    },
    classGroups: {
      "font-size": ["text-body","text-ui","text-meta","text-tiny","text-head","text-title"],
    },
  },
})

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}
