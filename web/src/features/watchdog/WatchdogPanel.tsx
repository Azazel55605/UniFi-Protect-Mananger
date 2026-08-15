import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, ApiError } from "@/lib/api";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Panel, PanelBody, PanelHeader } from "@/components/ui/panel";
import { Tally } from "@/components/ui/tally";
import type { WatchdogConfig } from "@/lib/types.gen";

/** Settings for the stall watchdog, plus what it has done recently. */
export function WatchdogPanel() {
  const queryClient = useQueryClient();
  const { data } = useQuery({ queryKey: ["watchdog"], queryFn: api.watchdog });
  const [draft, setDraft] = useState<WatchdogConfig | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (data && !draft) setDraft(data.config);
  }, [data, draft]);

  const save = useMutation({
    mutationFn: (c: WatchdogConfig) => api.saveWatchdog(c),
    onSuccess: (s) => {
      setDraft(s.config);
      setError(null);
      queryClient.invalidateQueries({ queryKey: ["watchdog"] });
    },
    onError: (e) => setError(e instanceof ApiError ? e.message : "Could not save"),
  });

  if (!draft || !data) return null;
  const set = (patch: Partial<WatchdogConfig>) => setDraft({ ...draft, ...patch });
  const dirty = JSON.stringify(draft) !== JSON.stringify(data.config);

  return (
    <Panel>
      <PanelHeader
        label="Backup watchdog"
        aside={
          <div className="flex items-center gap-2">
            <Tally state={!draft.enabled ? "idle" : data.stalled ? "bad" : "ok"} />
            <span className="data text-[11px] text-fg-faint">
              {!draft.enabled ? "off" : data.stalled ? "stalled" : "downloading"}
            </span>
          </div>
        }
      />
      <PanelBody className="space-y-5">
        <p className="text-sm text-fg-dim">{data.summary}</p>

        <label className="flex items-start gap-2 text-sm">
          <input
            type="checkbox"
            className="mt-1 accent-signal"
            checked={draft.enabled}
            onChange={(e) => set({ enabled: e.target.checked })}
          />
          <span>
            Watch for stalled downloads
            <span className="mt-0.5 block text-xs text-fg-faint">
              Compares events recorded against clips downloaded. A quiet night is
              not a stall — only events arriving without downloads is.
            </span>
          </span>
        </label>

        {draft.enabled && (
          <>
            <div className="flex flex-wrap items-end gap-4">
              <label className="flex flex-col gap-1">
                <span className="eyebrow">Allow behind by</span>
                <div className="flex items-center gap-2">
                  <Input
                    type="number"
                    min={5}
                    max={720}
                    className="data w-24"
                    value={draft.grace_minutes}
                    onChange={(e) => set({ grace_minutes: Number(e.target.value) || 30 })}
                  />
                  <span className="text-sm text-fg-dim">minutes</span>
                </div>
              </label>
            </div>

            <label className="flex items-start gap-2 text-sm">
              <input
                type="checkbox"
                className="mt-1 accent-signal"
                checked={draft.auto_restart}
                onChange={(e) => set({ auto_restart: e.target.checked })}
              />
              <span>
                Restart the backup container when stalled
                <span className="mt-0.5 block text-xs text-fg-faint">
                  Off by default. Only acts after the symptom has persisted for a
                  second grace period, and never more often than the cooldown.
                </span>
              </span>
            </label>

            {draft.auto_restart && (
              <label className="flex flex-col gap-1">
                <span className="eyebrow">Cooldown between restarts</span>
                <div className="flex items-center gap-2">
                  <Input
                    type="number"
                    min={5}
                    max={1440}
                    className="data w-24"
                    value={draft.restart_cooldown_minutes}
                    onChange={(e) =>
                      set({ restart_cooldown_minutes: Number(e.target.value) || 30 })
                    }
                  />
                  <span className="text-sm text-fg-dim">minutes</span>
                </div>
              </label>
            )}

            <div>
              <label className="eyebrow mb-1.5 block">Notify on stall (optional)</label>
              <Input
                className="data"
                placeholder="Falls back to the archive schedule's webhook"
                value={draft.webhook_url ?? ""}
                onChange={(e) => set({ webhook_url: e.target.value || null })}
              />
            </div>
          </>
        )}

        {error && <p className="text-sm text-bad">{error}</p>}

        <div className="flex items-center gap-3">
          <Button
            variant="primary"
            disabled={!dirty || save.isPending}
            onClick={() => save.mutate(draft)}
          >
            {save.isPending ? "Saving…" : "Save"}
          </Button>
          {dirty && (
            <Button variant="ghost" onClick={() => setDraft(data.config)}>
              Discard
            </Button>
          )}
        </div>

        {data.log.length > 0 && (
          <div className="border-t border-line pt-3">
            <div className="eyebrow mb-2">Recent activity</div>
            <ul className="space-y-1.5">
              {data.log.slice(0, 6).map((e, i) => (
                <li key={i} className="flex items-start gap-3">
                  <span className="data w-28 flex-none text-[11px] text-fg-faint">
                    {new Date(e.at * 1000).toLocaleString(undefined, {
                      month: "short",
                      day: "2-digit",
                      hour: "2-digit",
                      minute: "2-digit",
                      hourCycle: "h23",
                    })}
                  </span>
                  <span
                    className={cn(
                      "data w-20 flex-none text-[11px]",
                      e.action === "restarted" || e.action === "detected"
                        ? "text-warn"
                        : e.action === "failed"
                          ? "text-bad"
                          : "text-fg-faint",
                    )}
                  >
                    {e.action}
                  </span>
                  <span className="min-w-0 flex-1 text-xs text-fg-dim">{e.detail}</span>
                </li>
              ))}
            </ul>
          </div>
        )}
      </PanelBody>
    </Panel>
  );
}
