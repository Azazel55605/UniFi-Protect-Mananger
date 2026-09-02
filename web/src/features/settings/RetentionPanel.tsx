/**
 * The two retention knobs, editable outside the setup wizard.
 *
 * They lived only inside the wizard, which meant changing when clips become
 * archivable required walking back through container discovery and camera
 * selection — so in practice nobody changed it, and a threshold that was too
 * long looked like archiving being broken. They are one panel because the
 * question people arrive with is a single one ("when do my clips get packed
 * away?") even though the answer has two halves.
 */
import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ErrorNotice } from "@/components/ui/notice";
import { Panel, PanelBody, PanelHeader } from "@/components/ui/panel";
import type { Settings } from "@/lib/types.gen";

export function RetentionPanel({ compact = false }: { compact?: boolean }) {
  const queryClient = useQueryClient();
  const setup = useQuery({ queryKey: ["setup"], queryFn: api.setup });
  const saved = setup.data?.settings;

  const [days, setDays] = useState("");
  const [months, setMonths] = useState("");
  const [error, setError] = useState<unknown>(null);

  // Seeded from the server once, then left alone: re-seeding on every refetch
  // would overwrite what someone is in the middle of typing.
  useEffect(() => {
    if (!saved || days !== "") return;
    setDays(String(saved.archive_after_days));
    setMonths(String(saved.live_window_months));
  }, [saved, days]);

  const save = useMutation({
    mutationFn: (settings: Settings) => api.saveSettings(settings),
    onSuccess: () => {
      setError(null);
      // The due list is derived from these, so it is stale the moment they
      // change — and watching it update is how you confirm the change did
      // what you wanted.
      queryClient.invalidateQueries({ queryKey: ["setup"] });
      queryClient.invalidateQueries({ queryKey: ["archive"] });
      queryClient.invalidateQueries({ queryKey: ["events"] });
    },
    onError: setError,
  });

  if (!saved) return null;

  const daysNum = Number(days);
  const monthsNum = Number(months);
  const valid =
    Number.isInteger(daysNum) && daysNum >= 1 && daysNum <= 3650 &&
    Number.isInteger(monthsNum) && monthsNum >= 1 && monthsNum <= 120;
  const dirty =
    daysNum !== saved.archive_after_days || monthsNum !== saved.live_window_months;

  return (
    <Panel>
      <PanelHeader
        label="Retention"
        aside={
          <Button
            size="sm"
            variant="primary"
            disabled={!dirty || !valid || save.isPending}
            onClick={() =>
              save.mutate({
                ...saved,
                archive_after_days: daysNum,
                live_window_months: monthsNum,
              })
            }
          >
            {save.isPending ? "Saving…" : "Save"}
          </Button>
        }
      />
      <PanelBody className="space-y-4">
        <Field
          label="Archive clips older than"
          unit="days"
          value={days}
          min={1}
          max={3650}
          onChange={setDays}
          help="A camera-month is offered for archiving once its newest clip is this
                old. Whole months are still packed together, and the month currently
                being written to is never touched."
        />

        <Field
          label="Expect clips on disk for"
          unit="months"
          value={months}
          min={1}
          max={120}
          onChange={setMonths}
          help="How far back the index looks for files on disk. Anything older is
                assumed archived rather than checked, which is what keeps a sync from
                growing with your history. It does not decide when archiving runs."
        />

        {!compact && (
          <p className="text-xs text-fg-faint">
            Set the first one shorter than your disk can comfortably hold, and the
            second comfortably longer than the first — the backup service never
            deletes anything itself, so archiving is the only thing bounding growth.
          </p>
        )}

        {!valid && dirty && (
          <p className="text-xs text-bad">
            Days must be 1–3650 and months 1–120.
          </p>
        )}
        {error != null && <ErrorNotice error={error} />}
      </PanelBody>
    </Panel>
  );
}

function Field({
  label,
  unit,
  value,
  min,
  max,
  onChange,
  help,
}: {
  label: string;
  unit: string;
  value: string;
  min: number;
  max: number;
  onChange: (v: string) => void;
  help: string;
}) {
  return (
    <div>
      <label className="eyebrow mb-1.5 block">{label}</label>
      <div className="flex items-center gap-3">
        <Input
          type="number"
          min={min}
          max={max}
          className="data w-24"
          value={value}
          onChange={(e) => onChange(e.target.value)}
        />
        <span className="text-sm text-fg-dim">{unit}</span>
      </div>
      <p className="mt-2 text-xs text-fg-dim">{help}</p>
    </div>
  );
}
