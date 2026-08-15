import { useMemo, useState } from "react";
import { cn } from "@/lib/utils";
import type { EventRecord } from "@/lib/types.gen";

/**
 * A day as a single horizontal strip, one mark per event.
 *
 * The value here is density: a glance shows when things happened and when
 * nothing did, which a list of a hundred rows never conveys. Marks are placed
 * by time of day, not by index, so gaps are real gaps.
 */

const DAY = 86_400;

export function DayStrip({
  events,
  dayStart,
  selected,
  onSelect,
}: {
  events: EventRecord[];
  dayStart: number;
  selected: EventRecord | null;
  onSelect: (e: EventRecord) => void;
}) {
  const [hover, setHover] = useState<EventRecord | null>(null);

  // Marks are minimum-width, so a 6-second clip is still clickable — but a
  // long event should still look longer than a short one.
  const marks = useMemo(
    () =>
      events.map((e) => {
        const left = ((e.start - dayStart) / DAY) * 100;
        const width = Math.max(0.35, (e.duration / DAY) * 100);
        return { event: e, left, width };
      }),
    [events, dayStart],
  );

  return (
    <div>
      <div className="relative h-16 w-full overflow-hidden rounded-[3px] bg-ink-deep">
        {/* Three-hourly gridlines: enough to locate a time, few enough to
            stay out of the way. */}
        {Array.from({ length: 8 }, (_, i) => (i + 1) * 3).map((h) => (
          <div
            key={h}
            className="absolute top-0 bottom-0 w-px bg-line"
            style={{ left: `${(h / 24) * 100}%` }}
          />
        ))}

        {marks.map(({ event, left, width }) => {
          const live = event.status === "Live";
          const isSelected = selected?.id === event.id;
          return (
            <button
              key={event.id}
              onClick={() => live && onSelect(event)}
              onPointerEnter={() => setHover(event)}
              onPointerLeave={() => setHover(null)}
              title={`${new Date(event.start * 1000).toLocaleTimeString()} · ${event.camera}`}
              className={cn(
                "absolute top-2 bottom-2 rounded-[1px] transition-[opacity,transform]",
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

      <div className="mt-1 flex justify-between">
        {[0, 6, 12, 18, 24].map((h) => (
          <span key={h} className="data text-[10px] text-fg-faint">
            {String(h).padStart(2, "0")}:00
          </span>
        ))}
      </div>

      <p className="data mt-2 h-4 text-[11px] text-fg-dim">
        {hover
          ? `${new Date(hover.start * 1000).toLocaleTimeString(undefined, {
              hour: "2-digit",
              minute: "2-digit",
              second: "2-digit",
              hourCycle: "h23",
            })} · ${hover.camera} · ${hover.subtypes.join(", ") || hover.event_type}`
          : ""}
      </p>
    </div>
  );
}
