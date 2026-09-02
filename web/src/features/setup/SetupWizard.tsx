import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { ErrorNotice } from "@/components/ui/notice";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Panel, PanelBody, PanelHeader } from "@/components/ui/panel";
import { Tally } from "@/components/ui/tally";
import type { Settings } from "@/lib/types.gen";

/**
 * Setup is genuinely a sequence — each step depends on the one before it, and
 * the container has to be chosen before its mounts can be read — so numbering
 * the steps encodes something true rather than decorating them.
 */
const STEPS = ["Backup container", "Locations", "Cameras", "Retention", "Review"] as const;

export function SetupWizard({ onDone }: { onDone: () => void }) {
  const queryClient = useQueryClient();
  const [step, setStep] = useState(0);
  const [draft, setDraft] = useState<Settings | null>(null);
  const [error, setError] = useState<unknown>(null);

  const existing = useQuery({ queryKey: ["setup"], queryFn: api.setup });
  const discovery = useQuery({ queryKey: ["discover"], queryFn: api.discover });

  // Seed the form from what the container tells us, so the first thing the
  // user sees is a filled-in proposal rather than an empty form.
  useEffect(() => {
    if (draft || !existing.data || !discovery.data) return;
    const proposed = discovery.data.inspection?.proposed;
    const saved = existing.data.settings;

    setDraft({
      upb_container_id:
        saved.upb_container_id ?? discovery.data.containers[0]?.id ?? null,
      events_db_path: saved.events_db_path ?? proposed?.events_db_local_path ?? null,
      clip_path_prefix: saved.clip_path_prefix ?? proposed?.clip_path_prefix ?? null,
      camera_dirs:
        saved.camera_dirs.length > 0
          ? saved.camera_dirs
          : discovery.data.cameras.filter((c) => c.looks_like_camera).map((c) => c.dir_name),
      live_window_months: saved.live_window_months || 2,
      archive_after_days: saved.archive_after_days || 30,
      keep_sources_after_archive: saved.keep_sources_after_archive,
      setup_complete: saved.setup_complete,
    });
  }, [draft, existing.data, discovery.data]);

  const save = useMutation({
    mutationFn: (settings: Settings) => api.saveSettings(settings),
    onSuccess: () => {
      queryClient.invalidateQueries();
      onDone();
    },
    onError: setError,
  });

  if (!draft) {
    return (
      <div className="p-6">
        <p className="text-sm text-fg-dim">Reading your backup container…</p>
      </div>
    );
  }

  const set = (patch: Partial<Settings>) => setDraft({ ...draft, ...patch });
  const cameras = discovery.data?.cameras ?? [];
  const containers = discovery.data?.containers ?? [];

  const blocked = [
    !draft.upb_container_id,
    !draft.events_db_path || !draft.clip_path_prefix,
    draft.camera_dirs.length === 0,
    draft.live_window_months < 1 || draft.archive_after_days < 1,
    false,
  ];

  return (
    <div className="mx-auto max-w-3xl p-6">
      <div className="mb-6">
        <h1 className="text-lg font-semibold">Set up the console</h1>
        <p className="mt-1 text-sm text-fg-dim">
          Most of this is read from your backup container. Check it over and correct
          anything that looks wrong.
        </p>
      </div>

      <ol className="mb-6 flex flex-wrap gap-x-6 gap-y-2">
        {STEPS.map((label, i) => (
          <li key={label} className="flex items-center gap-2">
            <span
              className={cn(
                "data text-[11px]",
                i === step ? "text-signal-strong" : i < step ? "text-fg-dim" : "text-fg-faint",
              )}
            >
              {String(i + 1).padStart(2, "0")}
            </span>
            <span
              className={cn(
                "text-[13px]",
                i === step ? "text-fg" : "text-fg-faint",
              )}
            >
              {label}
            </span>
          </li>
        ))}
      </ol>

      <Panel>
        <PanelHeader label={STEPS[step] ?? ""} />
        <PanelBody className="min-h-[280px]">
          {step === 0 && (
            <div>
              <p className="mb-4 text-sm text-fg-dim">
                Found by image, so it doesn't matter what the container is called.
              </p>
              {containers.length === 0 ? (
                <p className="text-sm text-bad">
                  No backup container found. Check it is running, or set{" "}
                  <code className="data">PM_UPB_IMAGE</code> if you use a different image.
                </p>
              ) : (
                <div className="space-y-2">
                  {containers.map((c) => (
                    <label
                      key={c.id}
                      className={cn(
                        "flex cursor-pointer items-center gap-3 rounded-[3px] border p-3",
                        draft.upb_container_id === c.id
                          ? "border-signal bg-raised/70"
                          : "border-line hover:border-line-bright",
                      )}
                    >
                      <input
                        type="radio"
                        className="accent-signal"
                        checked={draft.upb_container_id === c.id}
                        onChange={() => set({ upb_container_id: c.id })}
                      />
                      <div className="min-w-0 flex-1">
                        <div className="text-sm">{c.name}</div>
                        <div className="data truncate text-fg-faint">{c.image}</div>
                      </div>
                      <Tally state={c.state === "running" ? "ok" : "bad"} />
                    </label>
                  ))}
                </div>
              )}
            </div>
          )}

          {step === 1 && (
            <div className="space-y-5">
              <div>
                <label className="eyebrow mb-1.5 block">Event database</label>
                <Input
                  className="data"
                  value={draft.events_db_path ?? ""}
                  placeholder="/backup/database/events.sqlite"
                  onChange={(e) => set({ events_db_path: e.target.value || null })}
                />
                <p className="mt-1.5 text-xs text-fg-faint">
                  Where this container can read the backup service's database.
                </p>
              </div>
              <div>
                <label className="eyebrow mb-1.5 block">Recorded path prefix</label>
                <Input
                  className="data"
                  value={draft.clip_path_prefix ?? ""}
                  placeholder="/data"
                  onChange={(e) => set({ clip_path_prefix: e.target.value || null })}
                />
                <p className="mt-1.5 text-xs text-fg-faint">
                  The backup service records clip paths as it sees them. This prefix is
                  replaced with your mounted clip directory.
                </p>
              </div>
              {(discovery.data?.notes.length ?? 0) > 0 && (
                <ul className="space-y-1">
                  {discovery.data?.notes.map((n) => (
                    <li key={n} className="text-xs text-warn">
                      {n}
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )}

          {step === 2 && (
            <div>
              <p className="mb-4 text-sm text-fg-dim">
                Directories holding dated folders of clips. Everything else is listed
                below in case the layout is unusual.
              </p>
              <div className="space-y-1.5">
                {cameras.map((c) => {
                  const checked = draft.camera_dirs.includes(c.dir_name);
                  return (
                    <label
                      key={c.dir_name}
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
                          set({
                            camera_dirs: checked
                              ? draft.camera_dirs.filter((d) => d !== c.dir_name)
                              : [...draft.camera_dirs, c.dir_name],
                          })
                        }
                      />
                      <span className="min-w-0 flex-1 truncate text-sm">{c.dir_name}</span>
                      <span className="data text-fg-faint">
                        {c.date_dirs > 0
                          ? `${c.date_dirs} days · ${c.clip_count}+ clips`
                          : "no dated folders"}
                      </span>
                    </label>
                  );
                })}
                {cameras.length === 0 && (
                  <p className="text-sm text-bad">
                    Nothing readable in the clip directory. Check the mount and the
                    container's group access.
                  </p>
                )}
              </div>
            </div>
          )}

          {step === 3 && (
            <div className="max-w-sm space-y-6">
              <div>
                <label className="eyebrow mb-1.5 block">Archive clips older than</label>
                <div className="flex items-center gap-3">
                  <Input
                    type="number"
                    min={1}
                    max={3650}
                    className="data w-24"
                    value={draft.archive_after_days}
                    onChange={(e) =>
                      set({ archive_after_days: Number(e.target.value) || 0 })
                    }
                  />
                  <span className="text-sm text-fg-dim">days</span>
                </div>
                <p className="mt-3 text-sm text-fg-dim">
                  Clips past this age are packed into one archive per camera per month
                  and removed from the live directory. This is the only setting
                  controlling disk growth — the backup service never deletes anything
                  itself.
                </p>
                <p className="mt-3 text-xs text-fg-faint">
                  Archives cover whole calendar months, so a month waits until its
                  newest clip passes this age. The month being written to right now is
                  never archived.
                </p>
              </div>

              <div>
                <label className="eyebrow mb-1.5 block">Expect clips on disk for</label>
                <div className="flex items-center gap-3">
                  <Input
                    type="number"
                    min={1}
                    max={120}
                    className="data w-24"
                    value={draft.live_window_months}
                    onChange={(e) =>
                      set({ live_window_months: Number(e.target.value) || 0 })
                    }
                  />
                  <span className="text-sm text-fg-dim">months</span>
                </div>
                <p className="mt-3 text-xs text-fg-faint">
                  How far back the index looks for files on disk before assuming they
                  have been archived. Keep it comfortably longer than the threshold
                  above; it does not itself trigger archiving.
                </p>
              </div>
            </div>
          )}

          {step === 4 && (
            <dl className="space-y-3">
              {[
                ["Container", containers.find((c) => c.id === draft.upb_container_id)?.name],
                ["Database", draft.events_db_path],
                ["Path prefix", draft.clip_path_prefix],
                ["Cameras", draft.camera_dirs.join(", ")],
                ["Archive after", `${draft.archive_after_days} days`],
                ["Live window", `${draft.live_window_months} months`],
              ].map(([k, v]) => (
                <div key={k} className="flex gap-4 border-b border-line pb-2.5">
                  <dt className="eyebrow w-28 flex-none pt-0.5">{k}</dt>
                  <dd className="data min-w-0 flex-1 break-all text-fg">{v || "—"}</dd>
                </div>
              ))}
              {error != null && <ErrorNotice error={error} />}
            </dl>
          )}
        </PanelBody>
      </Panel>

      <div className="mt-4 flex items-center justify-between">
        <Button
          variant="ghost"
          onClick={() => setStep((s) => Math.max(0, s - 1))}
          disabled={step === 0}
        >
          Back
        </Button>

        {step < STEPS.length - 1 ? (
          <Button
            variant="primary"
            onClick={() => setStep((s) => s + 1)}
            disabled={blocked[step]}
          >
            Continue
          </Button>
        ) : (
          <Button
            variant="primary"
            disabled={save.isPending}
            onClick={() => save.mutate({ ...draft, setup_complete: true })}
          >
            {save.isPending ? "Saving…" : "Finish setup"}
          </Button>
        )}
      </div>
    </div>
  );
}

/** Reusable summary of the saved configuration, used on the Settings page. */
export function SettingsSummary() {
  const { data } = useQuery({ queryKey: ["setup"], queryFn: api.setup });
  const rows = useMemo(() => data?.checks ?? [], [data]);

  return (
    <Panel>
      <PanelHeader label="Configuration" />
      <PanelBody className="space-y-2.5">
        {rows.map((c) => (
          // Stacked on a phone: these details are mostly filesystem paths, and
          // a fixed label column leaves them a dozen characters wide.
          <div key={c.name} className="flex items-start gap-3">
            <Tally state={c.ok ? "ok" : "bad"} className="mt-1.5" />
            <div className="min-w-0 flex-1 @lg:flex @lg:items-start @lg:gap-3">
              <span className="block text-sm text-fg-dim @lg:w-32 @lg:flex-none">{c.name}</span>
              <span className="data block min-w-0 break-all @lg:flex-1">{c.detail}</span>
            </div>
          </div>
        ))}
      </PanelBody>
    </Panel>
  );
}
