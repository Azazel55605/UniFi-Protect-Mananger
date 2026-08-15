import { useEffect, useState } from "react";
import { Link, useLocation } from "react-router-dom";
import { useProgress, useProgressPercent } from "@/lib/progress";
import { cn } from "@/lib/utils";
import { Tally } from "./ui/tally";

/**
 * A running job, visible from anywhere.
 *
 * Archiving several months takes minutes, and there is no reason to be stuck
 * on one page while it happens. The bubble stays out of the way until clicked;
 * the Archive page shows the full panel instead, so it hides itself there
 * rather than reporting the same thing twice.
 */
export function ProgressBubble() {
  const progress = useProgress();
  const pct = useProgressPercent(progress);
  const location = useLocation();
  const [open, setOpen] = useState(false);

  // Opening itself on completion would steal focus for something the user may
  // not be watching; closing it is the right way round.
  useEffect(() => {
    if (progress?.finished) setOpen(false);
  }, [progress?.finished]);

  if (!progress || location.pathname === "/archive") return null;

  const done = progress.finished;
  const failed = done && progress.status !== "Succeeded";

  return (
    <div className="fixed right-6 bottom-6 z-20 flex flex-col items-end gap-2">
      {open && (
        <div className="glass-overlay w-80 rounded-[3px] border border-line p-3 shadow-xl">
          <div className="mb-2 flex items-baseline justify-between">
            <span className="eyebrow">
              {progress.kind} · {progress.phase}
            </span>
            <Link
              to="/archive"
              className="data text-[11px] text-signal-strong hover:underline"
              onClick={() => setOpen(false)}
            >
              open
            </Link>
          </div>

          {progress.camera && (
            <p className="data mb-2 truncate text-[11px] text-fg-dim">
              {progress.camera} · {progress.month}
            </p>
          )}

          <MiniBar pct={pct.overall} />

          <p className="data mt-2 truncate text-[11px] text-fg-faint">
            {progress.message ??
              (progress.overall_total > 0
                ? `${progress.overall_done.toLocaleString()} / ${progress.overall_total.toLocaleString()} files`
                : "working…")}
          </p>
        </div>
      )}

      <button
        onClick={() => setOpen((o) => !o)}
        className={cn(
          "glass-overlay flex items-center gap-2.5 rounded-full border py-2 pr-4 pl-3 shadow-xl",
          "transition-colors hover:border-line-bright",
          failed ? "border-bad/50" : "border-line",
        )}
        title={done ? "Finished" : `${pct.overall}% complete`}
      >
        <Tally state={failed ? "bad" : done ? "ok" : "live"} />
        <span className="text-[13px]">
          {done ? (failed ? "Job failed" : "Job finished") : `${progress.kind} ${pct.overall}%`}
        </span>
      </button>
    </div>
  );
}

function MiniBar({ pct }: { pct: number }) {
  return (
    <div className="h-1 w-full overflow-hidden rounded-[2px] bg-raised">
      <div
        className="h-full bg-signal transition-[width] duration-200"
        style={{ width: `${Math.min(100, pct)}%` }}
      />
    </div>
  );
}
