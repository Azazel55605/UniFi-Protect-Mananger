import { NavLink, useLocation } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { api } from "@/lib/api";
import { cn } from "@/lib/utils";
import { Tally, type TallyState } from "./ui/tally";
import { ThemeSwitch } from "./AppearanceControls";

type NavItem = { to: string; label: string; ready: boolean };

/** Grouped the way the pipeline runs, not alphabetically. */
const groups: { label: string; items: NavItem[] }[] = [
  {
    label: "Footage",
    items: [
      { to: "/", label: "Overview", ready: true },
      { to: "/events", label: "Events", ready: true },
      { to: "/timeline", label: "Timeline", ready: false },
    ],
  },
  {
    label: "Storage",
    items: [
      { to: "/archive", label: "Archive", ready: true },
      { to: "/storage", label: "Capacity", ready: false },
    ],
  },
  {
    label: "System",
    items: [
      { to: "/logs", label: "Logs", ready: true },
      { to: "/settings", label: "Settings", ready: true },
    ],
  },
];

export function AppShell({ children }: { children: ReactNode }) {
  const location = useLocation();

  // Polled rather than pushed: the health of a backup pipeline changes on the
  // order of minutes, and a socket per tab would cost more than it's worth.
  const health = useQuery({
    queryKey: ["health"],
    queryFn: api.health,
    refetchInterval: 30_000,
  });

  // Warnings outrank a passing check. The connectivity checks can all pass
  // while something like a retention conflict is quietly destroying footage,
  // and a green light next to that would be worse than no light at all.
  const state: TallyState = health.isError
    ? "bad"
    : !health.data
      ? "idle"
      : !health.data.ok
        ? "bad"
        : health.data.warnings.length > 0
          ? "warn"
          : "ok";

  const summary = health.isError
    ? "Server unreachable"
    : !health.data
      ? "Checking"
      : (health.data.warnings[0] ??
        (health.data.ok ? "Pipeline healthy" : "Needs attention"));

  const title =
    groups.flatMap((g) => g.items).find((i) => i.to === location.pathname)?.label ?? "";

  return (
    <div className="flex h-screen overflow-hidden">
      <nav className="glass-chrome flex w-56 flex-none flex-col border-r border-line">
        <div className="flex items-center gap-2.5 border-b border-line px-4 py-4">
          <span className="tally bg-signal" aria-hidden />
          <span className="text-[13px] font-semibold tracking-[0.12em] uppercase">
            Protect
          </span>
        </div>

        <div className="flex-1 overflow-y-auto py-4">
          {groups.map((group) => (
            <div key={group.label} className="mb-5">
              <div className="eyebrow px-4 pb-2">{group.label}</div>
              {group.items.map((item) => (
                <NavLink
                  key={item.to}
                  to={item.to}
                  end={item.to === "/"}
                  className={({ isActive }) =>
                    cn(
                      "flex items-center gap-2.5 border-l-2 px-4 py-1.5 text-sm transition-colors",
                      isActive
                        ? "border-signal bg-raised text-fg"
                        : "border-transparent text-fg-dim hover:bg-raised/40 hover:text-fg",
                    )
                  }
                >
                  <span className="flex-1">{item.label}</span>
                  {!item.ready && (
                    <span className="data text-[10px] text-fg-faint">soon</span>
                  )}
                </NavLink>
              ))}
            </div>
          ))}
        </div>

        <div className="border-t border-line px-4 py-3">
          <div className="flex items-start gap-2">
            <Tally state={state} className="mt-1" />
            {/* Clamped: the sidebar reports that something needs attention,
                the Overview page is where it is explained in full. */}
            <span className="line-clamp-2 text-xs text-fg-dim" title={summary}>
              {summary}
            </span>
          </div>
        </div>
      </nav>

      <div className="flex min-w-0 flex-1 flex-col overflow-hidden">
        <header className="glass-chrome flex h-14 flex-none items-center justify-between border-b border-line px-6">
          <h1 className="text-sm font-semibold tracking-[0.08em] uppercase">{title}</h1>
          <div className="flex items-center gap-5">
            <StatusStrip />
            <ThemeSwitch />
          </div>
        </header>
        <main className="min-w-0 flex-1 overflow-y-auto p-6">{children}</main>
      </div>
    </div>
  );
}

/**
 * Three lights for the three things that have to be true for footage to be
 * arriving: the socket, the backup container, and the clip directory. It is
 * the same information as the Overview page, compressed to something you can
 * read from across the room.
 */
function StatusStrip() {
  const { data } = useQuery({ queryKey: ["health"], queryFn: api.health });
  if (!data) return null;

  const lights = [
    { label: "docker", ok: data.docker.ok },
    { label: "backup", ok: data.container.ok },
    { label: "clips", ok: data.backup_dir.ok },
  ];

  return (
    <div className="flex items-center gap-4">
      {lights.map((l) => (
        <div key={l.label} className="flex items-center gap-1.5" title={l.label}>
          <Tally state={l.ok ? "ok" : "bad"} />
          <span className="data text-[11px] text-fg-faint">{l.label}</span>
        </div>
      ))}
    </div>
  );
}
