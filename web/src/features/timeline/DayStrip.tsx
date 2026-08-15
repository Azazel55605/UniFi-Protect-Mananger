import { useMemo, useRef, useState } from "react";
import { cn } from "@/lib/utils";
import type { EventRecord } from "@/lib/types.gen";

/**
 * A day as a strip of marks, with a zoomable view window.
 *
 * Two strips, the way a video editor does it: an overview of the whole day
 * that never changes scale, and a detail strip showing the current window.
 * Zoomed in without the overview you lose all sense of where you are in the
 * day; the overview alone can't separate events seconds apart.
 */

const DAY = 86_400;
/** Half a minute across the strip is as far in as this is useful. */
const MIN_SPAN = 30;

export type View = { start: number; end: number };

export function DayStrip({
  events,
  dayStart,
  view,
  onViewChange,
  selected,
  onSelect,
}: {
  events: EventRecord[];
  dayStart: number;
  view: View;
  onViewChange: (v: View) => void;
  selected: EventRecord | null;
  onSelect: (e: EventRecord) => void;
}) {
  const [hover, setHover] = useState<EventRecord | null>(null);
  const stripRef = useRef<HTMLDivElement>(null);
  const drag = useRef<{ x: number; start: number } | null>(null);

  const dayEnd = dayStart + DAY;
  const span = view.end - view.start;
  const zoomed = span < DAY - 1;

  const clamp = (v: View): View => {
    const width = Math.min(DAY, Math.max(MIN_SPAN, v.end - v.start));
    let start = Math.max(dayStart, Math.min(v.start, dayEnd - width));
    return { start, end: start + width };
  };

  /** Zoom about a fixed point, so whatever is under the cursor stays there. */
  const zoomAt = (factor: number, atSeconds: number) => {
    const width = Math.min(DAY, Math.max(MIN_SPAN, span * factor));
    const ratio = (atSeconds - view.start) / span;
    onViewChange(clamp({ start: atSeconds - ratio * width, end: atSeconds - ratio * width + width }));
  };

  const timeAt = (clientX: number) => {
    const rect = stripRef.current?.getBoundingClientRect();
    if (!rect) return view.start;
    return view.start + ((clientX - rect.left) / rect.width) * span;
  };

  const marks = useMemo(
    () =>
      events
        .map((e) => ({
          event: e,
          left: ((e.start - view.start) / span) * 100,
          width: Math.max(0.25, (e.duration / span) * 100),
        }))
        // Off-screen marks are dropped rather than clamped to the edges, where
        // they would pile up and imply events that aren't in view.
        .filter((m) => m.left + m.width > -1 && m.left < 101),
    [events, view.start, span],
  );

  return (
    <div>
      {/* Detail strip */}
      <div
        ref={stripRef}
        onWheel={(e) => {
          e.preventDefault();
          zoomAt(e.deltaY > 0 ? 1.25 : 0.8, timeAt(e.clientX));
        }}
        onPointerDown={(e) => {
          drag.current = { x: e.clientX, start: view.start };
          e.currentTarget.setPointerCapture(e.pointerId);
        }}
        onPointerMove={(e) => {
          if (!drag.current) return;
          const rect = stripRef.current?.getBoundingClientRect();
          if (!rect) return;
          const shift = ((e.clientX - drag.current.x) / rect.width) * span;
          onViewChange(clamp({ start: drag.current.start - shift, end: drag.current.start - shift + span }));
        }}
        onPointerUp={() => (drag.current = null)}
        onPointerLeave={() => {
          drag.current = null;
          setHover(null);
        }}
        className={cn(
          "relative h-20 w-full touch-none overflow-hidden rounded-[3px] bg-ink-deep",
          zoomed ? "cursor-grab active:cursor-grabbing" : "cursor-default",
        )}
      >
        {ticks(view.start, view.end).map((t) => (
          <div
            key={t.at}
            className="absolute top-0 bottom-0 border-l border-line"
            style={{ left: `${((t.at - view.start) / span) * 100}%` }}
          >
            <span className="data absolute top-1 left-1 text-[10px] text-fg-faint">
              {t.label}
            </span>
          </div>
        ))}

        {marks.map(({ event, left, width }) => {
          const live = event.status === "Live";
          const isSelected = selected?.id === event.id;
          return (
            <button
              key={event.id}
              onClick={() => live && onSelect(event)}
              onPointerEnter={() => setHover(event)}
              title={`${clockOf(event.start)} · ${event.camera}`}
              className={cn(
                "absolute top-6 bottom-3 rounded-[1px] transition-colors",
                isSelected
                  ? "bg-signal"
                  : live
                    ? "bg-live/70 hover:bg-live"
                    : "bg-line-bright",
                !live && "cursor-default",
              )}
              style={{ left: `${left}%`, width: `${width}%`, minWidth: 3 }}
            />
          );
        })}
      </div>

      {/* Overview: the whole day, with the current window marked. Only worth
          showing once zoomed — at full extent it would duplicate the strip. */}
      {zoomed && (
        <div
          className="relative mt-1.5 h-6 w-full cursor-pointer overflow-hidden rounded-[2px] bg-ink-deep/60"
          onPointerDown={(e) => {
            const rect = e.currentTarget.getBoundingClientRect();
            const at = dayStart + ((e.clientX - rect.left) / rect.width) * DAY;
            onViewChange(clamp({ start: at - span / 2, end: at + span / 2 }));
          }}
          title="Click to jump"
        >
          {events.map((e) => (
            <div
              key={e.id}
              className={cn(
                "absolute top-1.5 bottom-1.5 w-0.5 rounded-[1px]",
                selected?.id === e.id ? "bg-signal" : "bg-live/40",
              )}
              style={{ left: `${((e.start - dayStart) / DAY) * 100}%` }}
            />
          ))}
          <div
            className="absolute top-0 bottom-0 border-x border-signal bg-signal/15"
            style={{
              left: `${((view.start - dayStart) / DAY) * 100}%`,
              width: `${(span / DAY) * 100}%`,
            }}
          />
        </div>
      )}

      <div className="mt-2 flex items-center gap-3">
        <span className="data text-[11px] text-fg-faint">
          {/* The end of a full day is the next midnight, which formats as
              "00:00" and reads as an empty range. */}
          {zoomed ? `${clockOf(view.start)} – ${clockOf(view.end)}` : "00:00 – 24:00"}
        </span>
        <span className="data flex-1 truncate text-[11px] text-fg-dim">
          {hover
            ? `${clockOf(hover.start)} · ${hover.camera} · ${
                hover.subtypes.join(", ") || hover.event_type
              } · ${Math.round(hover.duration)}s`
            : zoomed
              ? "drag to pan · scroll to zoom"
              : "scroll to zoom in"}
        </span>
      </div>
    </div>
  );
}

/** Gridlines at a spacing that suits the current span. */
function ticks(start: number, end: number) {
  const span = end - start;
  const step =
    span > 12 * 3600 ? 3 * 3600
    : span > 4 * 3600 ? 3600
    : span > 3600 ? 900
    : span > 600 ? 300
    : span > 120 ? 60
    : 15;

  const out: { at: number; label: string }[] = [];
  const first = Math.ceil(start / step) * step;
  for (let at = first; at < end; at += step) {
    out.push({ at, label: clockOf(at, step < 60) });
  }
  return out;
}

function clockOf(at: number, withSeconds = false) {
  return new Date(at * 1000).toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    ...(withSeconds ? { second: "2-digit" } : {}),
    hourCycle: "h23",
  });
}
