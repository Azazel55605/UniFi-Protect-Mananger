import { cn } from "@/lib/utils";

export type TallyState = "ok" | "bad" | "warn" | "live" | "idle";

const colors: Record<TallyState, string> = {
  ok: "bg-good",
  bad: "bg-bad",
  warn: "bg-warn",
  live: "bg-live tally-live",
  idle: "bg-line-bright",
};

/**
 * The status indicator this console is built around. Square, because that is
 * what equipment status lights look like — and because reusing one shape
 * everywhere means its meaning only has to be learned once.
 */
export function Tally({ state, className }: { state: TallyState; className?: string }) {
  return <span className={cn("tally", colors[state], className)} aria-hidden />;
}
