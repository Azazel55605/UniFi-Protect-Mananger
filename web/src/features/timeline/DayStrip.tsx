import { useCallback, useEffect, useMemo, useRef, useState } from "react";
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
/** Grab area for the window's edges, in pixels. */
const HANDLE = 10;

export type View = { start: number; end: number };

type Drag =
  | { mode: "pan"; x: number; start: number }
  | { mode: "move"; x: number; start: number; span: number }
  | { mode: "start"; end: number }
  | { mode: "end"; start: number };

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
  const overviewRef = useRef<HTMLDivElement>(null);
  const drag = useRef<Drag | null>(null);

  const dayEnd = dayStart + DAY;
  const span = view.end - view.start;
  const zoomed = span < DAY - 1;

  /// Pointer capture keeps a drag alive when the cursor leaves the element,
  /// but throws if the pointer is already gone — which must not take the
  /// whole gesture down with it.
  const capture = (e: React.PointerEvent<HTMLDivElement>) => {
    try {
      e.currentTarget.setPointerCapture(e.pointerId);
    } catch {
      /* the drag still works, it just stops at the element's edge */
    }
  };

  const clamp = useCallback(
    (v: View): View => {
      const width = Math.min(DAY, Math.max(MIN_SPAN, v.end - v.start));
      const start = Math.max(dayStart, Math.min(v.start, dayEnd - width));
      return { start, end: start + width };
    },
    [dayStart, dayEnd],
  );

  // The wheel handler is attached natively rather than through React, because
  // React registers wheel listeners as passive — `preventDefault` inside an
  // `onWheel` prop does nothing, and the page scrolls away underneath you
  // while you zoom. Kept in a ref so the listener always sees the current view
  // without being torn down and rebuilt on every change.
  const wheelState = useRef({ view, span, clamp, onViewChange });
  wheelState.current = { view, span, clamp, onViewChange };

  useEffect(() => {
    const el = stripRef.current;
    if (!el) return;

    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const { view, span, clamp, onViewChange } = wheelState.current;
      const rect = el.getBoundingClientRect();
      const at = view.start + ((e.clientX - rect.left) / rect.width) * span;
      const factor = e.deltaY > 0 ? 1.25 : 0.8;
      const width = Math.min(DAY, Math.max(MIN_SPAN, span * factor));
      const ratio = (at - view.start) / span;
      onViewChange(clamp({ start: at - ratio * width, end: at - ratio * width + width }));
    };

    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

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

  const timeInOverview = (clientX: number) => {
    const rect = overviewRef.current?.getBoundingClientRect();
    if (!rect) return dayStart;
    return dayStart + ((clientX - rect.left) / rect.width) * DAY;
  };

  const onOverviewDown = (e: React.PointerEvent<HTMLDivElement>) => {
    const rect = e.currentTarget.getBoundingClientRect();
    const pxPerSecond = rect.width / DAY;
    const leftPx = rect.left + (view.start - dayStart) * pxPerSecond;
    const rightPx = rect.left + (view.end - dayStart) * pxPerSecond;

    capture(e);

    // Edges take priority over the body, so a narrow window is still
    // resizable rather than only movable.
    if (Math.abs(e.clientX - leftPx) <= HANDLE) {
      drag.current = { mode: "start", end: view.end };
    } else if (Math.abs(e.clientX - rightPx) <= HANDLE) {
      drag.current = { mode: "end", start: view.start };
    } else if (e.clientX > leftPx && e.clientX < rightPx) {
      drag.current = { mode: "move", x: e.clientX, start: view.start, span };
    } else {
      // Outside the window: jump there, then keep dragging it around, so a
      // click and a drag are the same gesture rather than two behaviours.
      const at = timeInOverview(e.clientX);
      const next = clamp({ start: at - span / 2, end: at + span / 2 });
      onViewChange(next);
      drag.current = { mode: "move", x: e.clientX, start: next.start, span };
    }
  };

  const onOverviewMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d) return;
    const rect = e.currentTarget.getBoundingClientRect();
    const secondsPerPx = DAY / rect.width;

    if (d.mode === "move") {
      const shift = (e.clientX - d.x) * secondsPerPx;
      onViewChange(clamp({ start: d.start + shift, end: d.start + shift + d.span }));
    } else if (d.mode === "start") {
      const at = Math.min(timeInOverview(e.clientX), d.end - MIN_SPAN);
      onViewChange(clamp({ start: Math.max(dayStart, at), end: d.end }));
    } else if (d.mode === "end") {
      const at = Math.max(timeInOverview(e.clientX), d.start + MIN_SPAN);
      onViewChange(clamp({ start: d.start, end: Math.min(dayEnd, at) }));
    }
  };

  const endDrag = () => {
    drag.current = null;
  };

  return (
    <div>
      {/* Detail strip */}
      <div
        ref={stripRef}
        onPointerDown={(e) => {
          if (!zoomed) return;
          drag.current = { mode: "pan", x: e.clientX, start: view.start };
          capture(e);
        }}
        onPointerMove={(e) => {
          const d = drag.current;
          if (d?.mode !== "pan") return;
          const rect = stripRef.current?.getBoundingClientRect();
          if (!rect) return;
          const shift = ((e.clientX - d.x) / rect.width) * span;
          onViewChange(clamp({ start: d.start - shift, end: d.start - shift + span }));
        }}
        onPointerUp={endDrag}
        onPointerLeave={() => {
          endDrag();
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

      {/* Overview: the whole day, with the current window draggable by its
          middle and resizable by either edge. Only shown once zoomed — at full
          extent it would just duplicate the strip above. */}
      {zoomed && (
        <div
          ref={overviewRef}
          onPointerDown={onOverviewDown}
          onPointerMove={onOverviewMove}
          onPointerUp={endDrag}
          onPointerCancel={endDrag}
          className="relative mt-1.5 h-7 w-full touch-none overflow-hidden rounded-[2px] bg-ink-deep/60"
          title="Drag the window to move it, or its edges to resize"
        >
          {events.map((e) => (
            <div
              key={e.id}
              className={cn(
                "absolute top-2 bottom-2 w-0.5 rounded-[1px]",
                selected?.id === e.id ? "bg-signal" : "bg-live/40",
              )}
              style={{ left: `${((e.start - dayStart) / DAY) * 100}%` }}
            />
          ))}

          <div
            className="absolute top-0 bottom-0 cursor-grab bg-signal/15 active:cursor-grabbing"
            style={{
              left: `${((view.start - dayStart) / DAY) * 100}%`,
              width: `${(span / DAY) * 100}%`,
            }}
          />
          {/* Handles are drawn outside the window box so a very narrow window
              still presents something grabbable. */}
          <Handle x={((view.start - dayStart) / DAY) * 100} />
          <Handle x={((view.end - dayStart) / DAY) * 100} />
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
              ? "drag to pan · scroll to zoom · drag the window below to move or resize"
              : "scroll to zoom in"}
        </span>
      </div>
    </div>
  );
}

function Handle({ x }: { x: number }) {
  return (
    <div
      className="absolute top-0 bottom-0 w-1.5 -translate-x-1/2 cursor-ew-resize bg-signal"
      style={{ left: `${x}%` }}
    />
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
