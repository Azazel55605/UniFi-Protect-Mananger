import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

/** A rack panel: hairline border, near-square corners, no drop shadow. */
export function Panel({ className, children }: { className?: string; children: ReactNode }) {
  return (
    <section className={cn("glass border border-line rounded-[3px]", className)}>
      {children}
    </section>
  );
}

export function PanelHeader({
  label,
  aside,
}: {
  label: string;
  aside?: ReactNode;
}) {
  return (
    <header className="flex items-center justify-between gap-4 border-b border-line px-4 py-2.5">
      <h2 className="eyebrow">{label}</h2>
      {aside}
    </header>
  );
}

export function PanelBody({ className, children }: { className?: string; children: ReactNode }) {
  return <div className={cn("p-4", className)}>{children}</div>;
}
