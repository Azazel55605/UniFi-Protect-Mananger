import { useEffect, useState } from "react";
import { NavLink, useLocation } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { Menu, X } from "lucide-react";
import { api } from "@/lib/api";
import { cn } from "@/lib/utils";
import { Tally, type TallyState } from "./ui/tally";
import { ThemeSwitch } from "./AppearanceControls";
import { ProgressBubble } from "./ProgressBubble";

type NavItem = { to: string; label: string; ready: boolean };

/** Grouped the way the pipeline runs, not alphabetically. */
const groups: { label: string; items: NavItem[] }[] = [
  {
    label: "Footage",
    items: [
      { to: "/", label: "Overview", ready: true },
      { to: "/events", label: "Events", ready: true },
      { to: "/timeline", label: "Timeline", ready: true },
    ],
  },
  {
    label: "Storage",
    items: [
      { to: "/archive", label: "Archive", ready: true },
      { to: "/storage", label: "Capacity", ready: true },
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
  const [navOpen, setNavOpen] = useState(false);

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

  // Arriving somewhere is the end of navigating, so the drawer closes itself.
  // Leaving it open over the page you just asked for is the classic phone-nav
  // annoyance.
  useEffect(() => setNavOpen(false), [location.pathname]);

  // Escape closes it, and while it is open the page behind must not scroll —
  // otherwise a drag meant for the drawer moves the content underneath it.
  useEffect(() => {
    if (!navOpen) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setNavOpen(false);
    document.addEventListener("keydown", onKey);
    document.body.style.overflow = "hidden";
    return () => {
      document.removeEventListener("keydown", onKey);
      document.body.style.overflow = "";
    };
  }, [navOpen]);

  return (
    <div className="flex h-[100dvh] overflow-hidden">
      {/* The drawer's backdrop. Present only on small screens and only while
          open; on a desktop the nav is part of the layout and never covers
          anything. */}
      {navOpen && (
        <button
          aria-label="Close navigation"
          onClick={() => setNavOpen(false)}
          // Black rather than a theme token: the scrim has to darken the page
          // in both themes, and in light mode `ink-deep` is very nearly white,
          // so it dimmed nothing at all.
          className="fixed inset-0 z-30 bg-black/40 md:hidden"
        />
      )}

      <nav
        className={cn(
          "glass-chrome flex w-64 flex-none flex-col border-r border-line",
          // Off-canvas by default, part of the layout from md up. Transformed
          // rather than unmounted so it slides, and so its scroll position and
          // focus order survive being closed.
          "fixed inset-y-0 left-0 z-40 transition-transform duration-200",
          "md:static md:z-auto md:w-56 md:translate-x-0 md:transition-none",
          navOpen ? "translate-x-0" : "-translate-x-full",
        )}
        // The notch and the home indicator: without this the top of the nav
        // hides under the status bar in a fullscreen web app.
        style={{
          paddingTop: "env(safe-area-inset-top)",
          paddingBottom: "env(safe-area-inset-bottom)",
        }}
        aria-hidden={!navOpen ? undefined : false}
      >
        <div className="flex items-center gap-2.5 border-b border-line px-4 py-4">
          <span className="tally bg-signal" aria-hidden />
          <span className="text-[13px] font-semibold tracking-[0.12em] uppercase">
            Protect
          </span>
          <button
            onClick={() => setNavOpen(false)}
            aria-label="Close navigation"
            className="-mr-1 ml-auto grid h-8 w-8 place-items-center rounded-[3px] text-fg-dim hover:bg-raised hover:text-fg md:hidden"
          >
            <X size={16} />
          </button>
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
                      // Taller rows on touch: 1.5 units of padding is a
                      // comfortable mouse target and a poor thumb one.
                      "flex items-center gap-2.5 border-l-2 px-4 py-2.5 text-sm transition-colors md:py-1.5",
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
        <header
          className="glass-chrome flex h-14 flex-none items-center gap-3 border-b border-line px-4 md:px-6"
          style={{ paddingTop: "env(safe-area-inset-top)" }}
        >
          <button
            onClick={() => setNavOpen(true)}
            aria-label="Open navigation"
            aria-expanded={navOpen}
            className="-ml-1 grid h-9 w-9 flex-none place-items-center rounded-[3px] text-fg-dim hover:bg-raised hover:text-fg md:hidden"
          >
            <Menu size={18} />
          </button>

          <h1 className="min-w-0 flex-1 truncate text-sm font-semibold tracking-[0.08em] uppercase">
            {title}
          </h1>

          <div className="flex flex-none items-center gap-5">
            <StatusStrip />
            {/* One light instead of three when there is no room for labels:
                the same judgement, at the size that fits. */}
            <Tally state={state} className="md:hidden" />
            <ThemeSwitch />
          </div>
        </header>

        {/* A container, so the panels inside can lay themselves out against
            the width they actually have. Viewport breakpoints get this wrong
            in exactly one place and it is not a rare one: at 768px the sidebar
            reappears and takes 224px, so a "large" viewport hands its content
            barely 500px — and a table that unpacked itself at 640px viewport
            was then wider than the space it sat in. */}
        <main
          className="@container min-w-0 flex-1 overflow-y-auto p-4 md:p-6"
          style={{ paddingBottom: "max(1rem, env(safe-area-inset-bottom))" }}
        >
          {children}
        </main>
      </div>

      <ProgressBubble />
    </div>
  );
}

/**
 * Three lights for the three things that have to be true for footage to be
 * arriving: the socket, the backup container, and the clip directory. It is
 * the same information as the Overview page, compressed to something you can
 * read from across the room.
 *
 * Hidden below `md`, where the header has no room for three labelled lights —
 * the single tally beside it carries the same verdict.
 */
function StatusStrip() {
  const { data } = useQuery({ queryKey: ["health"], queryFn: api.health });
  if (!data) return null;

  const lights = [
    { label: "docker", ok: data.docker.ok },
    { label: "backup", ok: data.container.ok },
    { label: "clips", ok: data.backup_dir.ok },
    { label: "archive", ok: data.archive_dir.ok },
  ];

  return (
    <div className="hidden items-center gap-4 md:flex">
      {lights.map((l) => (
        <div key={l.label} className="flex items-center gap-1.5" title={l.label}>
          <Tally state={l.ok ? "ok" : "bad"} />
          <span className="data text-[11px] text-fg-faint">{l.label}</span>
        </div>
      ))}
    </div>
  );
}
