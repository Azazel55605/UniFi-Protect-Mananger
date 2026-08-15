import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { Panel, PanelBody, PanelHeader } from "@/components/ui/panel";
import { Tally } from "@/components/ui/tally";
import type { Check, IndexStats } from "@/lib/types.gen";

export function OverviewPage() {
  const health = useQuery({ queryKey: ["health"], queryFn: api.health, refetchInterval: 30_000 });
  const inspect = useQuery({ queryKey: ["inspect"], queryFn: api.inspect, retry: false });
  const index = useQuery({
    queryKey: ["index-stats"],
    queryFn: api.indexStats,
    refetchInterval: 30_000,
  });

  if (health.isError) {
    return (
      <Panel className="max-w-2xl">
        <PanelBody>
          <p className="text-sm text-bad">
            The server is not responding. If it was just restarted, this clears on its own.
          </p>
        </PanelBody>
      </Panel>
    );
  }

  const h = health.data;
  const checks: [string, Check | undefined][] = [
    ["Docker socket", h?.docker],
    ["Backup container", h?.container],
    ["Clip directory", h?.backup_dir],
  ];

  return (
    <div className="grid max-w-5xl gap-4 lg:grid-cols-2">
      <Panel className="lg:col-span-2">
        <PanelHeader
          label="Pipeline"
          aside={
            <span className="data text-[11px] text-fg-faint">
              {!h
                ? "checking"
                : !h.ok
                  ? "not connected"
                  : h.warnings.length > 0
                    ? `${h.warnings.length} to look at`
                    : "all clear"}
            </span>
          }
        />
        <PanelBody className="space-y-3">
          {checks.map(([label, check]) => (
            <div key={label} className="flex items-start gap-3">
              <Tally state={check ? (check.ok ? "ok" : "bad") : "idle"} className="mt-1.5" />
              <span className="w-36 flex-none text-sm">{label}</span>
              <span className="data min-w-0 flex-1 break-all text-fg-dim">
                {check?.detail ?? "checking…"}
              </span>
            </div>
          ))}
        </PanelBody>
      </Panel>

      {(h?.warnings.length ?? 0) > 0 && (
        <Panel className="lg:col-span-2 border-warn/40">
          <PanelHeader label="Attention" />
          <PanelBody className="space-y-2">
            {h?.warnings.map((w) => (
              <div key={w} className="flex items-start gap-3">
                <Tally state="warn" className="mt-1.5" />
                <span className="text-sm">{w}</span>
              </div>
            ))}
          </PanelBody>
        </Panel>
      )}

      {index.data && index.data.stats.total_events > 0 && (
        <Panel className="lg:col-span-2">
          <PanelHeader label="Footage" aside={<SyncAge stats={index.data.stats} />} />
          <PanelBody className="grid grid-cols-2 gap-4 sm:grid-cols-4">
            <Stat
              label="Last captured"
              value={formatLag(index.data.stats.backup_lag_secs)}
              tone={lagTone(index.data.stats.backup_lag_secs)}
              hint="Time since the newest event that produced a clip"
            />
            <Stat label="Events indexed" value={index.data.stats.total_events.toLocaleString()} />
            <Stat label="Clips available" value={index.data.stats.live_clips.toLocaleString()} />
            <Stat
              label="Not captured"
              value={(
                index.data.stats.pending_backfill + index.data.stats.never_backed_up
              ).toLocaleString()}
              tone={index.data.stats.never_backed_up > 0 ? "warn" : undefined}
              hint={
                `${index.data.stats.pending_backfill} may still be fetched · ` +
                `${index.data.stats.never_backed_up} never will be`
              }
            />
          </PanelBody>
        </Panel>
      )}

      <Panel>
        <PanelHeader label="Backup service" />
        <PanelBody>
          {inspect.data ? (
            <dl className="space-y-2.5">
              <Row k="Container" v={inspect.data.container.name} />
              <Row k="Image" v={inspect.data.container.image} />
              <Row
                k="Started"
                v={inspect.data.started_at?.replace("T", " ").slice(0, 19) ?? "—"}
              />
              <Row k="Restarts" v={String(inspect.data.restart_count)} />
              <Row k="Retention" v={inspect.data.retention ?? "not set"} />
              <Row k="Backfill window" v={inspect.data.missing_range ?? "not set"} />
            </dl>
          ) : (
            <p className="text-sm text-fg-dim">
              No backup container found. Pick one in Settings.
            </p>
          )}
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHeader label="Notes" />
        <PanelBody className="space-y-2">
          {(h?.info.length ?? 0) === 0 ? (
            <p className="text-sm text-fg-dim">Nothing worth flagging.</p>
          ) : (
            h?.info.map((i) => (
              <p key={i} className="text-sm text-fg-dim">
                {i}
              </p>
            ))
          )}
        </PanelBody>
      </Panel>
    </div>
  );
}

/**
 * Time since the last clip arrived. The single most useful number on this
 * page: a backup service can be running, logging happily, and quietly not
 * capturing anything, and nothing else here would notice.
 */
function formatLag(secs: number | null) {
  if (secs === null) return "never";
  if (secs < 90) return `${Math.round(secs)}s ago`;
  if (secs < 5400) return `${Math.round(secs / 60)}m ago`;
  if (secs < 172800) return `${Math.round(secs / 3600)}h ago`;
  return `${Math.round(secs / 86400)}d ago`;
}

/** Thresholds are deliberately loose — cameras are event-driven, so a quiet
 *  night is normal and only a long silence is suspicious. */
function lagTone(secs: number | null): "warn" | "bad" | undefined {
  if (secs === null) return "bad";
  if (secs > 172800) return "bad";
  if (secs > 43200) return "warn";
  return undefined;
}

function SyncAge({ stats }: { stats: IndexStats }) {
  if (stats.last_sync_error) {
    return <span className="data text-[11px] text-bad">index sync failing</span>;
  }
  if (!stats.last_sync) return null;
  const age = Math.max(0, Date.now() / 1000 - stats.last_sync);
  return (
    <span className="data text-[11px] text-fg-faint">
      indexed {age < 90 ? "just now" : `${Math.round(age / 60)}m ago`}
    </span>
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
    <div title={hint}>
      <div className="eyebrow mb-1">{label}</div>
      <div
        className={
          tone === "bad" ? "text-bad" : tone === "warn" ? "text-warn" : "text-fg"
        }
      >
        <span className="data text-lg">{value}</span>
      </div>
    </div>
  );
}

function Row({ k, v }: { k: string; v: string }) {
  return (
    <div className="flex gap-3">
      <dt className="eyebrow w-32 flex-none pt-0.5">{k}</dt>
      <dd className="data min-w-0 flex-1 break-all">{v}</dd>
    </div>
  );
}
