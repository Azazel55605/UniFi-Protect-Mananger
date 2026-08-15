import { Monitor, Moon, Sun } from "lucide-react";
import { ACCENTS, useAppearance, type Theme } from "@/lib/theme";
import { cn } from "@/lib/utils";

const THEMES: { id: Theme; label: string; Icon: typeof Sun }[] = [
  { id: "light", label: "Light", Icon: Sun },
  { id: "dark", label: "Dark", Icon: Moon },
  { id: "system", label: "Match system", Icon: Monitor },
];

/** Compact theme switch for the header. */
export function ThemeSwitch() {
  const { theme, setTheme } = useAppearance();

  return (
    <div
      className="flex items-center gap-0.5 rounded-[3px] border border-line p-0.5"
      role="group"
      aria-label="Theme"
    >
      {THEMES.map(({ id, label, Icon }) => (
        <button
          key={id}
          onClick={() => setTheme(id)}
          title={label}
          aria-label={label}
          aria-pressed={theme === id}
          className={cn(
            "grid h-6 w-6 place-items-center rounded-[2px] transition-colors",
            theme === id
              ? "bg-raised text-signal-strong"
              : "text-fg-faint hover:text-fg-dim",
          )}
        >
          <Icon size={13} strokeWidth={2} />
        </button>
      ))}
    </div>
  );
}

/** Full appearance controls for the Settings page. */
export function AppearancePicker() {
  const { theme, accent, setTheme, setAccent } = useAppearance();

  return (
    <div className="space-y-5">
      <div>
        <div className="eyebrow mb-2">Theme</div>
        <div className="flex flex-wrap gap-2">
          {THEMES.map(({ id, label, Icon }) => (
            <button
              key={id}
              onClick={() => setTheme(id)}
              aria-pressed={theme === id}
              className={cn(
                "flex items-center gap-2 rounded-[3px] border px-3 py-1.5 text-sm transition-colors",
                theme === id
                  ? "border-signal text-fg"
                  : "border-line text-fg-dim hover:border-line-bright",
              )}
            >
              <Icon size={14} strokeWidth={2} />
              {label}
            </button>
          ))}
        </div>
      </div>

      <div>
        <div className="eyebrow mb-2">Accent</div>
        <div className="flex flex-wrap gap-2">
          {ACCENTS.map((a) => (
            <button
              key={a.id}
              onClick={() => setAccent(a.id)}
              aria-pressed={accent === a.id}
              data-accent={a.id}
              className={cn(
                "flex items-center gap-2 rounded-[3px] border px-3 py-1.5 text-sm transition-colors",
                accent === a.id
                  ? "border-signal text-fg"
                  : "border-line text-fg-dim hover:border-line-bright",
              )}
            >
              <span className="tally bg-signal" aria-hidden />
              {a.label}
            </button>
          ))}
        </div>
        <p className="mt-2 text-xs text-fg-faint">
          Saved on this device. Status colours don't change — green, amber and red
          always mean the same thing.
        </p>
      </div>
    </div>
  );
}
