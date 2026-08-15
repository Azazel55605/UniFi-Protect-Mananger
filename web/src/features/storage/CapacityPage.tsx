import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { formatBytes, formatDays } from "@/lib/format";
import { cn } from "@/lib/utils";
import { Panel, PanelBody, PanelHeader } from "@/components/ui/panel";
import { Tally } from "@/components/ui/tally";
import type { CameraUsage, FilesystemUsage } from "@/lib/types.gen";
import { GrowthChart } from "./GrowthChart";

export function CapacityPage() {
  const storage = useQuery({
    queryKey: ["storage"],
    queryFn: api.storage,
    refetchInterval: 60_000,
  });
  const history = useQuery({
    queryKey: ["storage-history"],
    queryFn: () => api.storageHistory(30),
  });

  if (storage.isError) {
    return (
      <Panel className="max-w-2xl">
        <PanelBody>
          <p className="text-sm text-bad">Could not read storage usage.</p>
        </PanelBody>
      </Panel>
    );
  }

  const s = storage.data;
  const filesystems = uniqueFilesystems(s?.backup ?? null, s?.archive ?? null);
  const totalFootage = (s?.live_bytes ?? 0) + (s?.archive_bytes ?? 0);

  return (
    <div className="max-w-5xl space-y-4">
      <Panel>
        <PanelHeader
          label="Capacity"
          aside={
            s?.same_filesystem ? (
              <span className="data text-[11px] text-fg-faint">
                clips and archives share one filesystem
              </span>
            ) : undefined
          }
        />
        <PanelBody className="space-y-5">
          {filesystems.length === 0 ? (
            <p className="text-sm text-fg-dim">
              Neither directory is readable, so there is nothing to measure.
            </p>
          ) : (
            filesystems.map((fs) => <Meter key={fs.path} usage={fs} />)
          )}

          {s?.same_filesystem && (
            <p className="text-xs text-fg-faint">
              Because they share a filesystem, archiving only frees space once the
              originals are removed — packing alone briefly uses more.
            </p>
          )}
        </PanelBody>
      </Panel>

      <div className="grid gap-4 sm:grid-cols-3">
        <Stat label="Footage held" value={formatBytes(totalFootage)} />
        <Stat
          label="Growing by"
          value={
            s?.growth_bytes_per_day == null
              ? "measuring…"
              : `${formatBytes(s.growth_bytes_per_day)}/day`
          }
          hint="Measured across the sampled history, not estimated from clip sizes"
        />
        <Stat
          label="Space left"
          value={s?.days_until_full == null ? "—" : formatDays(s.days_until_full)}
          tone={
            s?.days_until_full != null && s.days_until_full < 60 ? "warn" : undefined
          }
          hint={
            s?.days_until_full == null
              ? "Needs a day of history, and only applies while usage is growing"
              : "At the current rate, and only if nothing is archived or deleted"
          }
        />
      </div>

      <Panel>
        <PanelHeader
          label="Footage over time"
          aside={
            <span className="data text-[11px] text-fg-faint">last 30 days</span>
          }
        />
        <PanelBody>
          <GrowthChart samples={history.data ?? []} />
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHeader label="By camera" />
        {(s?.cameras.length ?? 0) === 0 ? (
          <PanelBody>
            <p className="text-sm text-fg-dim">No cameras configured yet.</p>
          </PanelBody>
        ) : (
          <div>
            <div className="flex items-center gap-3 border-b border-line px-4 py-2">
              <span className="eyebrow flex-1">Camera</span>
              <span className="eyebrow w-24 text-right">Live</span>
              <span className="eyebrow w-24 text-right">Archived</span>
              <span className="eyebrow w-40 text-right">Share</span>
            </div>
            <ul className="divide-y divide-line">
              {s?.cameras.map((c) => (
                <CameraRow key={c.camera} usage={c} total={totalFootage} />
              ))}
            </ul>
          </div>
        )}
      </Panel>
    </div>
  );
}

/**
 * A capacity meter, not a chart: one filesystem, one proportion, read at a
 * glance. The colour is a state, so it uses the status palette rather than a
 * series colour.
 */
function Meter({ usage }: { usage: FilesystemUsage }) {
  const used = usage.total_bytes - usage.free_bytes;
  const pct = usage.total_bytes > 0 ? (used / usage.total_bytes) * 100 : 0;
  const tone = pct >= 90 ? "bad" : pct >= 75 ? "warn" : "ok";

  return (
    <div>
      <div className="mb-1.5 flex items-baseline gap-3">
        <Tally state={tone} />
        <span className="data flex-1 truncate text-fg-dim">{usage.path}</span>
        <span className="data text-fg">
          {formatBytes(used)} of {formatBytes(usage.total_bytes)}
        </span>
        <span className="data w-12 text-right text-fg-faint">{Math.round(pct)}%</span>
      </div>
      <div className="h-2 w-full overflow-hidden rounded-[2px] bg-raised">
        <div
          className={cn(
            "h-full transition-[width] duration-300",
            tone === "bad" ? "bg-bad" : tone === "warn" ? "bg-warn" : "bg-good",
          )}
          style={{ width: `${Math.min(100, pct)}%` }}
        />
      </div>
      <p className="mt-1 text-xs text-fg-faint">{formatBytes(usage.free_bytes)} free</p>
    </div>
  );
}

function CameraRow({ usage, total }: { usage: CameraUsage; total: number }) {
  const sum = usage.live_bytes + usage.archive_bytes;
  const share = total > 0 ? (sum / total) * 100 : 0;
  const livePart = sum > 0 ? (usage.live_bytes / sum) * 100 : 0;

  return (
    <li className="flex items-center gap-3 px-4 py-2.5">
      <span className="min-w-0 flex-1 truncate text-sm">{usage.camera}</span>
      <span className="data w-24 text-right text-fg-dim">
        {usage.live_bytes > 0 ? formatBytes(usage.live_bytes) : "—"}
      </span>
      <span className="data w-24 text-right text-fg-dim">
        {usage.archive_bytes > 0 ? formatBytes(usage.archive_bytes) : "—"}
      </span>
      <span className="flex w-40 items-center gap-2">
        {/* The same two series colours as the chart, so live and archived mean
            the same thing wherever they appear. */}
        <span className="flex h-2 flex-1 overflow-hidden rounded-[2px] bg-raised">
          <span
            style={{
              width: `${(share * livePart) / 100}%`,
              background: "var(--chart-live)",
            }}
          />
          <span
            style={{
              width: `${(share * (100 - livePart)) / 100}%`,
              background: "var(--chart-archive)",
            }}
          />
        </span>
        <span className="data w-9 text-right text-[11px] text-fg-faint">
          {Math.round(share)}%
        </span>
      </span>
    </li>
  );
}

function Stat({
  label,
  value,
  tone,
  hint,
}: {
  label: string;
  value: string;
  tone?: "warn" | "bad";
  hint?: string;
}) {
  return (
    <Panel>
      <PanelBody title={hint}>
        <div className="eyebrow mb-1">{label}</div>
        <div
          className={cn(
            "data text-lg",
            tone === "bad" ? "text-bad" : tone === "warn" ? "text-warn" : "text-fg",
          )}
        >
          {value}
        </div>
      </PanelBody>
    </Panel>
  );
}

/**
 * Two paths on one filesystem report the same free space; listing both would
 * imply twice the headroom that exists.
 */
function uniqueFilesystems(
  backup: FilesystemUsage | null,
  archive: FilesystemUsage | null,
): FilesystemUsage[] {
  const out: FilesystemUsage[] = [];
  for (const fs of [backup, archive]) {
    if (!fs) continue;
    if (out.some((seen) => seen.device === fs.device && fs.device !== 0)) continue;
    out.push(fs);
  }
  return out;
}
