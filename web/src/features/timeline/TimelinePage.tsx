import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { cn } from "@/lib/utils";
import { formatBytes } from "@/lib/format";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Panel, PanelBody, PanelHeader } from "@/components/ui/panel";
import type { EventRecord } from "@/lib/types.gen";
import { DayStrip, type View } from "./DayStrip";
import { ClipPlayer } from "./ClipPlayer";

const DAY = 86_400;
/** How much context to show either side when zooming to a clip. */
const CLIP_CONTEXT = 300;

function dayStart(offset: number) {
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  d.setDate(d.getDate() + offset);
  return d;
}

export function TimelinePage() {
  const [offset, setOffset] = useState(0);
  const [camera, setCamera] = useState<string | undefined>();
  const [search, setSearch] = useState("");
  const [oldestFirst, setOldestFirst] = useState(true);
  const [selected, setSelected] = useState<EventRecord | null>(null);
  const playerRef = useRef<HTMLDivElement>(null);

  const start = useMemo(() => dayStart(offset), [offset]);
  const startSecs = start.getTime() / 1000;
  const [view, setView] = useState<View>({ start: startSecs, end: startSecs + DAY });

  // Changing day resets the window; a zoom into yesterday afternoon means
  // nothing once you are looking at a different day.
  useEffect(() => {
    setView({ start: startSecs, end: startSecs + DAY });
    setSelected(null);
  }, [startSecs]);

  const cameras = useQuery({ queryKey: ["cameras"], queryFn: api.cameras });
  const events = useQuery({
    queryKey: ["timeline", startSecs, camera],
    queryFn: () =>
      api.events({
        from: startSecs,
        to: startSecs + DAY,
        camera_id: camera,
        // A day fits in one page, and the strip needs every event at once to
        // place marks correctly — paging would leave holes in the picture.
        limit: 500,
      }),
  });

  const chronological = useMemo(
    () => [...(events.data?.events ?? [])].reverse(),
    [events.data],
  );

  const matching = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return chronological;
    return chronological.filter((e) =>
      [e.camera, e.event_type, ...e.subtypes, clock(e.start)]
        .join(" ")
        .toLowerCase()
        .includes(q),
    );
  }, [chronological, search]);

  const ordered = useMemo(
    () => (oldestFirst ? matching : [...matching].reverse()),
    [matching, oldestFirst],
  );

  /** Selecting a clip zooms the strip to it and brings the player into view. */
  const select = (e: EventRecord) => {
    setSelected(e);
    const centre = e.start + e.duration / 2;
    const span = Math.max(e.duration + CLIP_CONTEXT, 120);
    setView({
      start: Math.max(startSecs, centre - span / 2),
      end: Math.min(startSecs + DAY, centre + span / 2),
    });
  };

  useEffect(() => {
    if (selected) {
      playerRef.current?.scrollIntoView({ behavior: "smooth", block: "nearest" });
    }
  }, [selected]);

  // Only playable clips can be stepped to; skipping to an archived one would
  // just close the player.
  const neighbours = useMemo(() => {
    const playable = ordered.filter((e) => e.status === "Live");
    const i = selected ? playable.findIndex((e) => e.id === selected.id) : -1;
    return {
      previous: i > 0 ? playable[i - 1] : undefined,
      next: i >= 0 && i < playable.length - 1 ? playable[i + 1] : undefined,
    };
  }, [ordered, selected]);

  const zoomBy = (factor: number) => {
    const centre = selected
      ? selected.start + selected.duration / 2
      : (view.start + view.end) / 2;
    const width = Math.min(DAY, Math.max(30, (view.end - view.start) * factor));
    setView({
      start: Math.max(startSecs, centre - width / 2),
      end: Math.min(startSecs + DAY, Math.max(startSecs + width, centre + width / 2)),
    });
  };

  const zoomed = view.end - view.start < DAY - 1;

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

          <select
            value={camera ?? ""}
            onChange={(e) => setCamera(e.target.value || undefined)}
            className="h-8 min-w-40 rounded-[3px] border border-line bg-ink/60 px-2 text-sm text-fg"
            aria-label="Camera"
          >
            <option value="">All cameras</option>
            {(cameras.data ?? []).map((c) => (
              <option key={c.camera_id} value={c.camera_id}>
                {c.display_name}
              </option>
            ))}
          </select>

          <Input
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="Search camera, detection or time"
            className="h-8 w-64"
            aria-label="Search clips"
          />

          <Button
            size="sm"
            variant="ghost"
            onClick={() => setOldestFirst((v) => !v)}
            title="Change the order clips are listed in"
          >
            {oldestFirst ? "Oldest first" : "Newest first"}
          </Button>

          <span className="data ml-auto text-[11px] text-fg-faint">
            {events.isLoading
              ? "…"
              : search
                ? `${ordered.length} of ${chronological.length}`
                : `${ordered.length} events`}
          </span>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHeader
          label="Day"
          aside={
            <div className="flex items-center gap-1.5">
              <Button size="sm" variant="ghost" onClick={() => zoomBy(0.5)} title="Zoom in">
                +
              </Button>
              <Button size="sm" variant="ghost" onClick={() => zoomBy(2)} title="Zoom out">
                −
              </Button>
              {zoomed && (
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => setView({ start: startSecs, end: startSecs + DAY })}
                >
                  Whole day
                </Button>
              )}
            </div>
          }
        />
        <PanelBody>
          <DayStrip
            events={matching}
            dayStart={startSecs}
            view={view}
            onViewChange={setView}
            selected={selected}
            onSelect={select}
          />
        </PanelBody>
      </Panel>

      <div ref={playerRef}>
        {selected && (
          <ClipPlayer
            event={selected}
            onClose={() => setSelected(null)}
            // Neighbours follow the list you are looking at, so "next" means
            // the next one shown rather than the next in some hidden order.
            onPrevious={neighbours.previous ? () => select(neighbours.previous!) : undefined}
            onNext={neighbours.next ? () => select(neighbours.next!) : undefined}
          />
        )}
      </div>

      <Panel>
        <PanelHeader
          label="Clips"
          aside={
            <span className="data text-[11px] text-fg-faint">
              {oldestFirst ? "oldest first" : "newest first"}
            </span>
          }
        />
        {ordered.length === 0 ? (
          <PanelBody className="py-10 text-center">
            <p className="text-sm text-fg-dim">
              {events.isLoading
                ? "Loading…"
                : search
                  ? "Nothing matches that search."
                  : "Nothing was recorded on this day."}
            </p>
          </PanelBody>
        ) : (
          <PanelBody className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4">
            {ordered.map((e) => (
              <ClipTile
                key={e.id}
                event={e}
                active={selected?.id === e.id}
                onSelect={() => select(e)}
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

  return (
    <button
      onClick={live ? onSelect : undefined}
      disabled={!live}
      className={cn(
        "overflow-hidden rounded-[3px] border text-left transition-colors",
        active ? "border-signal" : "border-line hover:border-line-bright",
        !live && "opacity-60",
      )}
    >
      <div className="relative aspect-video w-full bg-ink-deep">
        {live ? (
          // Lazily: a busy day is a hundred thumbnails, and each one is an
          // ffmpeg call the first time it is asked for.
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
        <span className="data absolute right-1 bottom-1 rounded-[2px] bg-ink/80 px-1 text-[10px] text-fg">
          {formatDuration(event.duration)}
        </span>
      </div>

      <div className="flex items-center gap-2 px-2.5 py-1.5">
        <span className="data text-[11px] text-fg">{clock(event.start)}</span>
        <span className="min-w-0 flex-1 truncate text-[11px] text-fg-dim">
          {event.camera}
        </span>
        {event.size_bytes != null && (
          <span className="data text-[10px] text-fg-faint">
            {formatBytes(event.size_bytes)}
          </span>
        )}
      </div>
    </button>
  );
}

function clock(at: number) {
  return new Date(at * 1000).toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hourCycle: "h23",
  });
}

export function formatDuration(secs: number) {
  if (secs < 60) return `${Math.round(secs)}s`;
  const m = Math.floor(secs / 60);
  return `${m}:${String(Math.round(secs % 60)).padStart(2, "0")}`;
}
