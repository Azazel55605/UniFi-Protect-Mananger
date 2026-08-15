import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, ApiError } from "@/lib/api";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Panel, PanelBody, PanelHeader } from "@/components/ui/panel";
import { Tally } from "@/components/ui/tally";
import type { ArchiveEntry, CameraMonth, DueEntry, RunStatus } from "@/lib/types.gen";
import { useProgress } from "./useProgress";
import { SchedulePanel } from "./SchedulePanel";

export function ArchivePage() {
  const queryClient = useQueryClient();
  const progress = useProgress();
  const [selected, setSelected] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);

  const overview = useQuery({ queryKey: ["archive"], queryFn: api.archive });
  const runs = useQuery({ queryKey: ["archive-runs"], queryFn: api.archiveRuns });

  const busy = progress !== null && !progress.finished;

  const start = useMutation({
    mutationFn: (dryRun: boolean) =>
      api.startArchive({
        targets: keysToTargets(selected, overview.data?.due ?? []),
        dry_run: dryRun,
      }),
    onSuccess: () => {
      setError(null);
      queryClient.invalidateQueries({ queryKey: ["archive-runs"] });
    },
    onError: (e) => setError(e instanceof ApiError ? e.message : "Could not start"),
  });

  const due = (overview.data?.due ?? []).filter((d) => !d.blocked);
  const blocked = (overview.data?.due ?? []).filter((d) => d.blocked);
  const archives = overview.data?.archives ?? [];
  const missing = overview.data?.missing_archives ?? [];

  return (
    <div className="max-w-5xl space-y-4">
      {progress && <ProgressPanel />}

      {missing.length > 0 && (
        <Panel className="border-bad/40">
          <PanelHeader label="Archives missing" />
          <PanelBody className="space-y-1.5">
            <p className="mb-2 text-sm text-bad">
              These were archived, but the file is no longer where it was written.
            </p>
            {missing.map((m) => (
              <div key={`${m.camera}/${m.month}`} className="data text-fg-dim">
                {m.camera} · {m.month}
              </div>
            ))}
          </PanelBody>
        </Panel>
      )}

      <Panel>
        <PanelHeader
          label="Ready to archive"
          aside={
            <div className="flex items-center gap-2">
              <Button
                size="sm"
                variant="ghost"
                disabled={busy || due.length === 0}
                onClick={() => start.mutate(true)}
                title="Report what would happen without writing or deleting anything"
              >
                Dry run
              </Button>
              <Button
                size="sm"
                variant="primary"
                disabled={busy || due.length === 0}
                onClick={() => start.mutate(false)}
              >
                {selected.length > 0 ? `Archive ${selected.length}` : "Archive all"}
              </Button>
            </div>
          }
        />
        <PanelBody className="space-y-1.5">
          {due.length === 0 ? (
            <p className="text-sm text-fg-dim">
              {blocked.length > 0
                ? "Nothing left to archive — the months below are held back for the reasons shown."
                : "Nothing is old enough to archive yet. Months move here once every day in them is past the live window."}
            </p>
          ) : (
            due.map((d) => {
              const key = `${d.camera}/${d.month}`;
              const checked = selected.includes(key);
              return (
                <label
                  key={key}
                  className={cn(
                    "flex cursor-pointer items-center gap-3 rounded-[3px] border p-2.5",
                    checked ? "border-line-bright bg-raised/70" : "border-line",
                  )}
                >
                  <input
                    type="checkbox"
                    className="accent-signal"
                    checked={checked}
                    onChange={() =>
                      setSelected((s) =>
                        checked ? s.filter((k) => k !== key) : [...s, key],
                      )
                    }
                  />
                  <span className="flex-1 text-sm">{d.camera}</span>
                  <span className="data text-fg-dim">{d.month}</span>
                  <span className="data w-40 text-right text-fg-faint">
                    {d.file_count.toLocaleString()} clips · {formatBytes(d.bytes)}
                  </span>
                </label>
              );
            })
          )}

          {blocked.map((d) => (
            <BlockedRow key={`${d.camera}/${d.month}`} entry={d} />
          ))}

          {error && <p className="text-sm text-bad">{error}</p>}
        </PanelBody>
      </Panel>

      <SchedulePanel />

      <Panel>
        <PanelHeader
          label="Archives"
          aside={
            <span className="data text-[11px] text-fg-faint">
              {archives.length} · {formatBytes(overview.data?.total_bytes ?? 0)}
            </span>
          }
        />
        {archives.length === 0 ? (
          <PanelBody>
            <p className="text-sm text-fg-dim">No archives yet.</p>
          </PanelBody>
        ) : (
          <ul className="divide-y divide-line">
            {archives.map((a) => (
              <ArchiveRow key={`${a.camera}/${a.month}`} entry={a} busy={busy} />
            ))}
          </ul>
        )}
      </Panel>

      <Panel>
        <PanelHeader label="Run history" />
        {(runs.data ?? []).length === 0 ? (
          <PanelBody>
            <p className="text-sm text-fg-dim">Nothing has run yet.</p>
          </PanelBody>
        ) : (
          <ul className="divide-y divide-line">
            {runs.data?.slice(0, 12).map((r) => (
              <li key={r.id} className="flex items-center gap-3 px-4 py-2.5">
                <Tally state={runTone(r.status)} className="flex-none" />
                <span className="data w-32 flex-none text-fg-dim">
                  {new Date(r.started * 1000).toLocaleString(undefined, {
                    month: "short",
                    day: "2-digit",
                    hour: "2-digit",
                    minute: "2-digit",
                    hourCycle: "h23",
                  })}
                </span>
                <span className="w-20 flex-none text-sm capitalize">
                  {r.kind.toLowerCase()}
                </span>
                <span className="data w-16 flex-none text-[11px] text-fg-faint">
                  {r.dry_run ? "dry run" : r.scheduled ? "scheduled" : "manual"}
                </span>
                <span className="min-w-0 flex-1 truncate text-sm text-fg-dim">
                  {r.message ?? "—"}
                </span>
                {r.failed_files.length > 0 && (
                  <span
                    className="data flex-none text-[11px] text-bad"
                    title={r.failed_files.join("\n")}
                  >
                    {r.failed_files.length} failed
                  </span>
                )}
              </li>
            ))}
          </ul>
        )}
      </Panel>
    </div>
  );
}

function ProgressPanel() {
  const progress = useProgress();
  if (!progress) return null;

  const pct =
    progress.overall_total > 0
      ? Math.round((progress.overall_done / progress.overall_total) * 100)
      : 0;
  const filePct =
    progress.files_total > 0
      ? Math.round((progress.files_done / progress.files_total) * 100)
      : 0;

  return (
    <Panel className="border-signal/40">
      <PanelHeader
        label={progress.finished ? "Finished" : `${progress.kind} — ${progress.phase}`}
        aside={
          progress.camera ? (
            <span className="data text-[11px] text-fg-faint">
              {progress.camera} · {progress.month}
            </span>
          ) : undefined
        }
      />
      <PanelBody className="space-y-3">
        {/* Two bars, matching how the job actually decomposes: the current
            camera-month, and the run as a whole. */}
        <Bar label="This month" pct={filePct} done={progress.files_done} total={progress.files_total} />
        <Bar label="Overall" pct={pct} done={progress.overall_done} total={progress.overall_total} />
        <p className="data truncate text-[11px] text-fg-faint">
          {progress.message ?? progress.current_file ?? ""}
        </p>
      </PanelBody>
    </Panel>
  );
}

function Bar({
  label,
  pct,
  done,
  total,
}: {
  label: string;
  pct: number;
  done: number;
  total: number;
}) {
  return (
    <div>
      <div className="mb-1 flex items-baseline justify-between">
        <span className="eyebrow">{label}</span>
        <span className="data text-[11px] text-fg-faint">
          {done.toLocaleString()} / {total.toLocaleString()}
        </span>
      </div>
      <div className="h-1.5 w-full overflow-hidden rounded-[2px] bg-raised">
        <div
          className="h-full bg-signal transition-[width] duration-200"
          style={{ width: `${Math.min(100, pct)}%` }}
        />
      </div>
    </div>
  );
}

function BlockedRow({ entry }: { entry: DueEntry }) {
  return (
    <div className="flex items-center gap-3 rounded-[3px] border border-line/60 p-2.5 opacity-70">
      <Tally state="idle" />
      <span className="flex-1 text-sm">{entry.camera}</span>
      <span className="data text-fg-dim">{entry.month}</span>
      <span className="w-72 text-right text-[11px] text-fg-faint">{entry.blocked}</span>
    </div>
  );
}

function ArchiveRow({ entry, busy }: { entry: ArchiveEntry; busy: boolean }) {
  const queryClient = useQueryClient();
  const [error, setError] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);
  const target: CameraMonth = { camera: entry.camera, month: entry.month };

  const refresh = () => queryClient.invalidateQueries({ queryKey: ["archive"] });
  const fail = (e: unknown) => setError(e instanceof ApiError ? e.message : "Failed");

  const verify = useMutation({ mutationFn: () => api.verifyArchive(target), onError: fail });
  const restore = useMutation({
    mutationFn: () => api.restore(target),
    onSuccess: () => setConfirming(false),
    onError: (e) => {
      setConfirming(false);
      fail(e);
    },
  });
  const pin = useMutation({
    mutationFn: (pinned: boolean) => api.setPinned(entry.camera, entry.month, pinned),
    onSuccess: refresh,
    onError: fail,
  });

  return (
    <li className="px-4 py-2.5">
      <div className="flex items-center gap-3">
        <Tally
          state={entry.verify_ok === false ? "bad" : entry.verify_ok ? "ok" : "idle"}
          className="flex-none"
        />
        <span className="w-40 flex-none truncate text-sm">{entry.camera}</span>
        <span className="data w-20 flex-none text-fg-dim">{entry.month}</span>
        <span className="data w-24 flex-none text-right text-fg-faint">
          {formatBytes(entry.size_bytes)}
        </span>
        <span className="data min-w-0 flex-1 truncate text-[11px] text-fg-faint">
          {entry.unrecorded
            ? "not created by this app"
            : entry.file_count > 0
              ? `${entry.file_count.toLocaleString()} files`
              : ""}
        </span>

        {entry.pinned && (
          <button
            className="data flex-none text-[11px] text-warn"
            onClick={() => pin.mutate(false)}
            title="Restored to live, so scheduled runs skip it. Click to release."
          >
            pinned
          </button>
        )}

        <Button size="sm" variant="ghost" disabled={busy} onClick={() => verify.mutate()}>
          Verify
        </Button>

        {confirming ? (
          <div className="flex flex-none items-center gap-2">
            <Button size="sm" variant="primary" onClick={() => restore.mutate()}>
              Confirm
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setConfirming(false)}>
              Cancel
            </Button>
          </div>
        ) : (
          <Button
            size="sm"
            variant="ghost"
            disabled={busy}
            onClick={() => setConfirming(true)}
          >
            Restore
          </Button>
        )}
      </div>

      {confirming && (
        <p className="mt-2 text-xs text-fg-dim">
          Unpacks {entry.month} back into the live directory ({formatBytes(entry.size_bytes)}).
          The archive is kept, and the month is pinned so a scheduled run won't
          immediately archive it again.
        </p>
      )}
      {error && <p className="mt-2 text-xs text-bad">{error}</p>}
    </li>
  );
}

function runTone(status: RunStatus) {
  if (status === "Succeeded") return "ok" as const;
  if (status === "Running") return "live" as const;
  if (status === "Interrupted") return "warn" as const;
  return "bad" as const;
}

function keysToTargets(keys: string[], due: DueEntry[]): CameraMonth[] {
  if (keys.length === 0) return [];
  return due
    .filter((d) => keys.includes(`${d.camera}/${d.month}`))
    .map((d) => ({ camera: d.camera, month: d.month }));
}

export function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
}
