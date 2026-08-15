import { cva, type VariantProps } from "class-variance-authority";
import type { ButtonHTMLAttributes } from "react";
import { cn } from "@/lib/utils";

const button = cva(
  "inline-flex items-center justify-center gap-2 rounded-[3px] border font-medium " +
    "transition-colors disabled:opacity-40 disabled:pointer-events-none whitespace-nowrap",
  {
    variants: {
      variant: {
        primary:
          "bg-signal text-signal-contrast border-signal hover:bg-signal/85 hover:border-signal/85",
        default:
          "glass text-fg border-line hover:border-line-bright",
        ghost: "bg-transparent text-fg-dim border-transparent hover:text-fg hover:bg-raised",
        danger: "bg-transparent text-bad border-bad/40 hover:bg-bad/10 hover:border-bad",
      },
      size: {
        sm: "h-8 px-3 text-[13px]",
        md: "h-9 px-4 text-sm",
      },
    },
    defaultVariants: { variant: "default", size: "md" },
  },
);

export function Button({
  className,
  variant,
  size,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & VariantProps<typeof button>) {
  return <button className={cn(button({ variant, size }), className)} {...props} />;
}
