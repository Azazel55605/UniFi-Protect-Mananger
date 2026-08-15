import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Panel, PanelBody, PanelHeader } from "@/components/ui/panel";
import { Tally, type TallyState } from "@/components/ui/tally";
import type { ClipStatus, EventQuery, EventRecord } from "@/lib/types.gen";

const PAGE_SIZE = 100;

/** What each clip state means, in the reader's terms rather than the schema's. */
const STATUS: Record<ClipStatus, { label: string; tone: TallyState; hint: string }> = {
  Live: { label: "Available", tone: "ok", hint: "The clip is on disk" },
  Archived: {
    label: "Archived",
    tone: "idle",
    hint: "Aged out of the live window and packed into an archive",
  },
  Vanished: {
    label: "Clip missing",
    tone: "bad",
    hint: "Recorded and backed up, but the file is no longer there",
  },
  PendingBackfill: {
    label: "Not captured yet",
    tone: "warn",
    hint: "The backup service may still fetch this one",
  },
  NeverBackedUp: {
    label: "Never captured",
    tone: "bad",
    hint: "Too old to be backfilled — this footage is gone",
  },
};

export function EventsPage() {
  const [filters, setFilters] = useState<EventQuery>({});
  const [page, setPage] = useState(0);

  const meta = useQuery({ queryKey: ["index-stats"], queryFn: api.indexStats });
  const cameras = useQuery({ queryKey: ["cameras"], queryFn: api.cameras });

  const query = useMemo(
    () => ({ ...filters, limit: PAGE_SIZE, offset: page * PAGE_SIZE }),
    [filters, page],
  );
  const events = useQuery({
    queryKey: ["events", query],
    queryFn: () => api.events(query),
    placeholderData: (prev) => prev,
  });

  // Changing a filter must reset paging, or you land on page 4 of a result set
  // that now has one page and see nothing.
  const set = (patch: Partial<EventQuery>) => {
    setFilters((f) => ({ ...f, ...patch }));
    setPage(0);
  };

  const active = Object.values(filters).some((v) => v !== undefined && v !== null);
  const total = events.data?.total ?? 0;
  const pages = Math.max(1, Math.ceil(total / PAGE_SIZE));

  return (
    <div className="max-w-6xl space-y-4">
      <Panel>
        <PanelBody className="flex flex-wrap items-end gap-3 py-3">
          <Select
            label="Camera"
            value={filters.camera_id ?? ""}
            onChange={(v) => set({ camera_id: v || undefined })}
            options={(cameras.data ?? []).map((c) => ({
              value: c.camera_id,
              label: `${c.display_name} (${c.event_count})`,
            }))}
          />
          <Select
            label="Event type"
            value={filters.event_type ?? ""}
            onChange={(v) => set({ event_type: v || undefined })}
            options={(meta.data?.event_types ?? []).map((t) => ({ value: t, label: t }))}
          />
          <Select
            label="Detected"
            value={filters.subtype ?? ""}
            onChange={(v) => set({ subtype: v || undefined })}
            options={(meta.data?.stats.distinct_subtypes ?? []).map((t) => ({
              value: t,
              label: t,
            }))}
          />
          <Select
            label="Clip"
            value={filters.status ?? ""}
            onChange={(v) => set({ status: (v as ClipStatus) || undefined })}
            options={Object.entries(STATUS).map(([value, s]) => ({
              value,
              label: s.label,
            }))}
          />

          {active && (
            <Button size="sm" variant="ghost" onClick={() => { setFilters({}); setPage(0); }}>
              Clear
            </Button>
          )}

          <div className="ml-auto data text-[11px] text-fg-faint">
            {events.isLoading ? "…" : `${total.toLocaleString()} events`}
          </div>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHeader
          label="Events"
          aside={
            pages > 1 ? (
              <div className="flex items-center gap-2">
                <Button size="sm" variant="ghost" disabled={page === 0} onClick={() => setPage((p) => p - 1)}>
                  Newer
                </Button>
                <span className="data text-[11px] text-fg-faint">
                  {page + 1} / {pages}
                </span>
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={page + 1 >= pages}
                  onClick={() => setPage((p) => p + 1)}
                >
                  Older
                </Button>
              </div>
            ) : undefined
          }
        />

        {events.isError ? (
          <PanelBody>
            <p className="text-sm text-bad">Could not load events.</p>
          </PanelBody>
        ) : total === 0 && !events.isLoading ? (
          <PanelBody className="py-10 text-center">
            <p className="text-sm text-fg-dim">
              {active
                ? "No events match these filters."
                : "No events indexed yet. The index rebuilds every minute after setup."}
            </p>
          </PanelBody>
        ) : (
          <ul className="divide-y divide-line">
            {events.data?.events.map((e) => (
              <EventRow key={e.id} event={e} />
            ))}
          </ul>
        )}
      </Panel>
    </div>
  );
}

function EventRow({ event }: { event: EventRecord }) {
  const status = STATUS[event.status];
  const when = new Date(event.start * 1000);

  return (
    <li className="flex items-center gap-4 px-4 py-2.5 hover:bg-raised/40">
      <Tally state={status.tone} className="flex-none" />

      {/* 24-hour, always. An AM/PM suffix wraps the column and costs the
          camera name its space, and timecodes are what this kind of console
          deals in anyway. */}
      <time
        className="data w-32 flex-none whitespace-nowrap text-fg-dim"
        dateTime={when.toISOString()}
      >
        {when.toLocaleString(undefined, {
          month: "short",
          day: "2-digit",
          hour: "2-digit",
          minute: "2-digit",
          second: "2-digit",
          hourCycle: "h23",
        })}
      </time>

      {/* The camera name takes the slack, since names vary in length far more
          than anything else in the row. */}
      <span className="min-w-0 flex-1 truncate text-sm">{event.camera}</span>

      {/* Detections get a fixed column so rows stay a uniform height. Most
          events have one; a few have two, and anything beyond that is
          summarised rather than clipped mid-word. */}
      <span className="flex w-36 flex-none items-center gap-1.5 overflow-hidden">
        {event.subtypes.length > 0 ? (
          <>
            {event.subtypes.slice(0, 2).map((s) => (
              <span
                key={s}
                className="flex-none rounded-[2px] border border-line px-1.5 py-px text-[11px] text-fg-dim"
              >
                {s}
              </span>
            ))}
            {event.subtypes.length > 2 && (
              <span
                className="data flex-none text-[11px] text-fg-faint"
                title={event.subtypes.join(", ")}
              >
                +{event.subtypes.length - 2}
              </span>
            )}
          </>
        ) : (
          <span className="data truncate text-[11px] text-fg-faint">{event.event_type}</span>
        )}
      </span>

      <span className="data w-16 flex-none text-right text-fg-faint">
        {formatDuration(event.duration)}
      </span>

      <span
        className={cn(
          "data w-28 flex-none text-right text-[11px]",
          event.status === "Live" ? "text-fg-faint" : "text-fg-dim",
        )}
        title={status.hint}
      >
        {event.status === "Live" ? formatSize(event.size_bytes) : status.label}
      </span>
    </li>
  );
}

function formatDuration(secs: number) {
  if (secs < 60) return `${Math.round(secs)}s`;
  const m = Math.floor(secs / 60);
  return `${m}m ${Math.round(secs % 60)}s`;
}

function formatSize(bytes: number | null) {
  if (bytes === null || bytes === undefined) return "—";
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function Select({
  label,
  value,
  onChange,
  options,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  options: { value: string; label: string }[];
}) {
  return (
    <label className="flex flex-col gap-1">
      <span className="eyebrow">{label}</span>
      <select
        value={value}
        onChange={(e) => onChange(e.target.value)}
        disabled={options.length === 0}
        className={cn(
          "h-8 min-w-40 rounded-[3px] border border-line bg-ink/60 px-2 text-sm text-fg",
          "disabled:opacity-40",
        )}
      >
        <option value="">Any</option>
        {options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
    </label>
  );
}
