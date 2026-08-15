import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { ErrorNotice } from "@/components/ui/notice";
import { Panel, PanelBody, PanelHeader } from "@/components/ui/panel";
import { Tally } from "@/components/ui/tally";

/**
 * Every browser currently able to sign in as you, and a way to cut them off.
 *
 * With one account and a fourteen-day cookie, a session on a device you no
 * longer have stays valid for a fortnight, and changing the password does not
 * touch it — sessions are independent of the hash that created them. This is
 * the only way to end one.
 */
export function SessionsPanel() {
  const queryClient = useQueryClient();
  const [error, setError] = useState<unknown>(null);

  const sessions = useQuery({ queryKey: ["sessions"], queryFn: api.sessions });

  const revoke = useMutation({
    mutationFn: api.revokeOtherSessions,
    onMutate: () => setError(null),
    onSuccess: (list) => queryClient.setQueryData(["sessions"], list),
    onError: setError,
  });

  const rows = sessions.data ?? [];
  const others = rows.filter((s) => !s.current).length;

  return (
    <Panel>
      <PanelHeader
        label="Sessions"
        aside={
          <span className="data text-[11px] text-fg-faint">
            {rows.length === 0 ? "" : `${rows.length} signed in`}
          </span>
        }
      />
      <PanelBody className="space-y-3">
        {sessions.isError && <ErrorNotice error={sessions.error} />}

        <ul className="divide-y divide-line">
          {rows.map((s) => (
            <li key={s.id} className="flex items-start gap-3 py-2.5 first:pt-0">
              <Tally state={s.current ? "live" : "idle"} className="mt-1.5" />
              <div className="min-w-0 flex-1">
                <p className="truncate text-sm text-fg">
                  {s.user_agent ?? "Unknown browser"}
                  {s.current && (
                    <span className="ml-2 text-xs text-signal">this device</span>
                  )}
                </p>
                <p className="data mt-0.5 text-[11px] text-fg-faint">
                  {s.address ?? "unknown address"} · active {when(s.last_seen)} · signed
                  in {when(s.created)} · expires {when(s.expires)}
                </p>
              </div>
            </li>
          ))}
        </ul>

        {rows.length === 0 && !sessions.isError && (
          <p className="text-sm text-fg-dim">
            {sessions.isLoading ? "Loading…" : "No sessions."}
          </p>
        )}

        {error != null && <ErrorNotice error={error} />}

        <div className="flex items-center gap-3 pt-1">
          <Button
            variant="danger"
            disabled={others === 0 || revoke.isPending}
            onClick={() => revoke.mutate()}
          >
            {revoke.isPending ? "Signing out…" : "Sign out other sessions"}
          </Button>
          <span className="text-xs text-fg-dim">
            {others === 0
              ? "Nothing else is signed in."
              : `${others} other session${others === 1 ? "" : "s"} would end immediately.`}
          </span>
        </div>
      </PanelBody>
    </Panel>
  );
}

/**
 * A timestamp phrased the way you would say it out loud.
 *
 * Exact dates are the wrong unit here: what makes a row recognisable is "a
 * minute ago" versus "three weeks ago", not the calendar date it happened on.
 */
function when(epochSeconds: number) {
  const delta = epochSeconds - Date.now() / 1000;
  const abs = Math.abs(delta);

  const units: [Intl.RelativeTimeFormatUnit, number][] = [
    ["second", 60],
    ["minute", 3600],
    ["hour", 86_400],
    ["day", 2_592_000],
    ["month", 31_536_000],
  ];

  const format = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
  let divisor = 1;
  for (const [unit, limit] of units) {
    if (abs < limit) return format.format(Math.round(delta / divisor), unit);
    divisor = limit;
  }
  return format.format(Math.round(delta / 31_536_000), "year");
}
