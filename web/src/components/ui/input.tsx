import type { InputHTMLAttributes } from "react";
import { cn } from "@/lib/utils";

export function Input({ className, ...props }: InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      className={cn(
        "h-9 w-full rounded-[3px] border border-line bg-ink/60 px-3 text-sm text-fg",
        "placeholder:text-fg-faint focus:border-line-bright outline-none",
        className,
      )}
      {...props}
    />
  );
}
