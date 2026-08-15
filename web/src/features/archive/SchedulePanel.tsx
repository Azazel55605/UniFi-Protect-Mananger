import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, ApiError } from "@/lib/api";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Panel, PanelBody, PanelHeader } from "@/components/ui/panel";
import { Tally } from "@/components/ui/tally";
import type { Schedule, ScheduleKind } from "@/lib/types.gen";

/**
 * The server stores hours in UTC, because a container cannot reliably work out
 * the viewer's timezone. The browser can, so the conversion happens here — the
 * user picks the hour they mean and never sees UTC.
 */
function utcHourToLocal(utcHour: number): number {
  const d = new Date();
  d.setUTCHours(utcHour, 0, 0, 0);
  return d.getHours();
}

function localHourToUtc(localHour: number): number {
  const d = new Date();
  d.setHours(localHour, 0, 0, 0);
  return d.getUTCHours();
}

const KINDS: { id: ScheduleKind; label: string; hint: string }[] = [
  { id: "Off", label: "Manual only", hint: "Nothing runs unless you start it" },
  { id: "Monthly", label: "Monthly", hint: "Once a month, on a day you choose" },
  {
    id: "Daily",
    label: "Daily",
    hint: "Checks every day. Cheap when nothing is due, and a missed month doesn't wait another four weeks",
  },
];

export function SchedulePanel() {
  const queryClient = useQueryClient();
  const { data } = useQuery({ queryKey: ["schedule"], queryFn: api.schedule });
  const [draft, setDraft] = useState<Schedule | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (data && !draft) setDraft(data);
  }, [data, draft]);

  const save = useMutation({
    mutationFn: (s: Schedule) => api.saveSchedule(s),
    onSuccess: (saved) => {
      setDraft(saved);
      setError(null);
      queryClient.invalidateQueries({ queryKey: ["schedule"] });
    },
    onError: (e) => setError(e instanceof ApiError ? e.message : "Could not save"),
  });

  if (!draft) return null;

  const set = (patch: Partial<Schedule>) => setDraft({ ...draft, ...patch });
  const dirty = JSON.stringify(draft) !== JSON.stringify(data);

  return (
    <Panel>
      <PanelHeader
        label="Schedule"
        aside={
          <div className="flex items-center gap-2">
            <Tally state={draft.kind === "Off" ? "idle" : "ok"} />
            <span className="data text-[11px] text-fg-faint">
              {draft.kind === "Off"
                ? "manual only"
                : data?.next_run
                  ? `next ${new Date(data.next_run * 1000).toLocaleString(undefined, {
                      month: "short",
                      day: "2-digit",
                      hour: "2-digit",
                      minute: "2-digit",
                      hourCycle: "h23",
                    })}`
                  : ""}
            </span>
          </div>
        }
      />
      <PanelBody className="space-y-5">
        <div className="flex flex-wrap gap-2">
          {KINDS.map((k) => (
            <button
              key={k.id}
              title={k.hint}
              onClick={() => set({ kind: k.id })}
              className={cn(
                "rounded-[3px] border px-3 py-1.5 text-sm transition-colors",
                draft.kind === k.id
                  ? "border-signal text-fg"
                  : "border-line text-fg-dim hover:border-line-bright",
              )}
            >
              {k.label}
            </button>
          ))}
        </div>

        {draft.kind !== "Off" && (
          <div className="flex flex-wrap items-end gap-4">
            {draft.kind === "Monthly" && (
              <label className="flex flex-col gap-1">
                <span className="eyebrow">Day of month</span>
                <Input
                  type="number"
                  min={1}
                  max={31}
                  className="data w-24"
                  value={draft.day}
                  onChange={(e) => set({ day: Number(e.target.value) || 1 })}
                />
              </label>
            )}

            <label className="flex flex-col gap-1">
              <span className="eyebrow">Hour</span>
              <Input
                type="number"
                min={0}
                max={23}
                className="data w-24"
                value={utcHourToLocal(draft.hour)}
                onChange={(e) => set({ hour: localHourToUtc(Number(e.target.value) || 0) })}
              />
            </label>

            <label className="flex items-center gap-2 pb-2 text-sm">
              <input
                type="checkbox"
                className="accent-signal"
                checked={draft.catch_up}
                onChange={(e) => set({ catch_up: e.target.checked })}
              />
              Run late if the time was missed
            </label>
          </div>
        )}

        {draft.kind === "Monthly" && draft.day > 28 && (
          <p className="text-xs text-fg-faint">
            Months shorter than {draft.day} days run on their last day instead of skipping.
          </p>
        )}

        <div>
          <label className="eyebrow mb-1.5 block">Notify on failure (optional)</label>
          <Input
            className="data"
            placeholder="http://homeassistant.local:8123/api/webhook/…"
            value={draft.webhook_url ?? ""}
            onChange={(e) => set({ webhook_url: e.target.value || null })}
          />
          <p className="mt-1.5 text-xs text-fg-faint">
            A failed run posts JSON here. Without it, a failure is only visible in run
            history — which means only when you happen to look.
          </p>
        </div>

        {error && <p className="text-sm text-bad">{error}</p>}

        <div className="flex items-center gap-3">
          <Button variant="primary" disabled={!dirty || save.isPending} onClick={() => save.mutate(draft)}>
            {save.isPending ? "Saving…" : "Save schedule"}
          </Button>
          {dirty && (
            <Button variant="ghost" onClick={() => data && setDraft(data)}>
              Discard
            </Button>
          )}
          {data?.last_run && (
            <span className="data ml-auto text-[11px] text-fg-faint">
              last attempt {new Date(data.last_run * 1000).toLocaleString()}
            </span>
          )}
        </div>
      </PanelBody>
    </Panel>
  );
}
