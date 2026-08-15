import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Panel, PanelBody, PanelHeader } from "@/components/ui/panel";
import type { EventRecord } from "@/lib/types.gen";
import { DayStrip } from "./DayStrip";
import { ClipPlayer } from "./ClipPlayer";

/** Local midnight for a day offset from today. */
function dayStart(offset: number) {
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  d.setDate(d.getDate() + offset);
  return d;
}

export function TimelinePage() {
  const [offset, setOffset] = useState(0);
  const [camera, setCamera] = useState<string | undefined>();
  const [selected, setSelected] = useState<EventRecord | null>(null);

  const start = useMemo(() => dayStart(offset), [offset]);
  const end = useMemo(() => dayStart(offset + 1), [offset]);

  const cameras = useQuery({ queryKey: ["cameras"], queryFn: api.cameras });
  const events = useQuery({
    queryKey: ["timeline", start.getTime(), camera],
    queryFn: () =>
      api.events({
        from: start.getTime() / 1000,
        to: end.getTime() / 1000,
        camera_id: camera,
        // A day of events fits in one page; the strip needs all of them at
        // once to place marks correctly, so paging would break the picture.
        limit: 500,
      }),
  });

  const all = events.data?.events ?? [];
  // The API returns newest first, which is right for a feed and wrong for a
  // timeline — a day reads left to right.
  const ordered = useMemo(() => [...all].reverse(), [all]);
  const playable = ordered.filter((e) => e.status === "Live");

  return (
    <div className="max-w-6xl space-y-4">
      <Panel>
        <PanelBody className="flex flex-wrap items-center gap-3 py-3">
          <div className="flex items-center gap-1">
            <Button size="sm" variant="ghost" onClick={() => setOffset((o) => o - 1)}>
              ‹
            </Button>
            <span className="data w-44 text-center text-fg">
              {start.toLocaleDateString(undefined, {
                weekday: "short",
                day: "2-digit",
                month: "short",
                year: "numeric",
              })}
            </span>
            <Button
              size="sm"
              variant="ghost"
              disabled={offset >= 0}
              onClick={() => setOffset((o) => o + 1)}
            >
              ›
            </Button>
          </div>

          {offset !== 0 && (
            <Button size="sm" variant="ghost" onClick={() => setOffset(0)}>
              Today
            </Button>
          )}

          <label className="flex items-center gap-2">
            <span className="eyebrow">Camera</span>
            <select
              value={camera ?? ""}
              onChange={(e) => setCamera(e.target.value || undefined)}
              className="h-8 min-w-44 rounded-[3px] border border-line bg-ink/60 px-2 text-sm text-fg"
            >
              <option value="">All</option>
              {(cameras.data ?? []).map((c) => (
                <option key={c.camera_id} value={c.camera_id}>
                  {c.display_name}
                </option>
              ))}
            </select>
          </label>

          <span className="data ml-auto text-[11px] text-fg-faint">
            {events.isLoading ? "…" : `${ordered.length} events`}
          </span>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHeader
          label="Day"
          aside={
            <span className="data text-[11px] text-fg-faint">
              00:00 – 24:00 · click a mark to play
            </span>
          }
        />
        <PanelBody>
          <DayStrip
            events={ordered}
            dayStart={start.getTime() / 1000}
            selected={selected}
            onSelect={setSelected}
          />
        </PanelBody>
      </Panel>

      {selected && (
        <ClipPlayer event={selected} onClose={() => setSelected(null)} />
      )}

      <Panel>
        <PanelHeader
          label="Clips"
          aside={
            playable.length !== ordered.length ? (
              <span className="data text-[11px] text-fg-faint">
                {ordered.length - playable.length} archived or missing
              </span>
            ) : undefined
          }
        />
        {ordered.length === 0 ? (
          <PanelBody className="py-10 text-center">
            <p className="text-sm text-fg-dim">
              {events.isLoading ? "Loading…" : "Nothing was recorded on this day."}
            </p>
          </PanelBody>
        ) : (
          <PanelBody className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4">
            {ordered.map((e) => (
              <ClipTile
                key={e.id}
                event={e}
                active={selected?.id === e.id}
                onSelect={() => setSelected(e)}
              />
            ))}
          </PanelBody>
        )}
      </Panel>
    </div>
  );
}

function ClipTile({
  event,
  active,
  onSelect,
}: {
  event: EventRecord;
  active: boolean;
  onSelect: () => void;
}) {
  const live = event.status === "Live";
  const time = new Date(event.start * 1000).toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hourCycle: "h23",
  });

  return (
    <button
      onClick={live ? onSelect : undefined}
      disabled={!live}
      className={cn(
        "group overflow-hidden rounded-[3px] border text-left transition-colors",
        active ? "border-signal" : "border-line hover:border-line-bright",
        !live && "opacity-60",
      )}
    >
      <div className="relative aspect-video w-full bg-ink-deep">
        {live ? (
          // Lazily, because a busy day is a hundred thumbnails and each one
          // is an ffmpeg call the first time it is asked for.
          <img
            src={`/api/media/${encodeURIComponent(event.id)}/thumb`}
            alt=""
            loading="lazy"
            className="h-full w-full object-cover"
          />
        ) : (
          <span className="data absolute inset-0 grid place-items-center text-[11px] text-fg-faint">
            {event.status === "Archived" ? "in archive" : "no clip"}
          </span>
        )}
      </div>

      <div className="flex items-center gap-2 px-2.5 py-1.5">
        <span className="data text-[11px] text-fg">{time}</span>
        <span className="min-w-0 flex-1 truncate text-[11px] text-fg-dim">
          {event.camera}
        </span>
        {event.subtypes[0] && (
          <span className="data text-[10px] text-fg-faint">{event.subtypes[0]}</span>
        )}
      </div>
    </button>
  );
}
