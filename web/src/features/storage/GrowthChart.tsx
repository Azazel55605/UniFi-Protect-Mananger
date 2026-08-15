import { useEffect, useMemo, useRef, useState } from "react";
import type { StorageSample } from "@/lib/types.gen";
import { formatBytes } from "@/lib/format";

/**
 * Footage on disk over time, split into live and archived.
 *
 * Stacked, because the two parts sum to something meaningful — total footage
 * held — and the split is the story: archiving should show live falling as
 * archived rises, with the total growing more slowly than live alone would.
 *
 * Free space is deliberately *not* plotted here. It is a different measure on
 * a different scale, and putting it on this axis would mean either a second
 * y-scale or a misleading comparison. It belongs in the capacity meters.
 */

type Point = { x: number; live: number; archive: number; at: number };

/**
 * The chart is drawn at its real size rather than scaled into place.
 *
 * A fixed 840-unit viewBox stretched to fit is fine for the curves and wrong
 * for everything else: shrunk onto a phone, a 10px axis label lands at about
 * four physical pixels and stops being a label. Measuring the container and
 * drawing one SVG unit per CSS pixel keeps the type at the size it says.
 */
function useWidth(ref: React.RefObject<HTMLElement | null>) {
  const [width, setWidth] = useState(840);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const observer = new ResizeObserver(([entry]) => {
      if (entry) setWidth(Math.max(280, entry.contentRect.width));
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, [ref]);

  return width;
}

export function GrowthChart({ samples }: { samples: StorageSample[] }) {
  const svgRef = useRef<SVGSVGElement>(null);
  const boxRef = useRef<HTMLElement>(null);
  const [hover, setHover] = useState<number | null>(null);

  const W = useWidth(boxRef);
  const narrow = W < 520;
  const H = narrow ? 150 : 200;
  // The left gutter holds a byte label; it can be tighter when the numbers are
  // shorter, but not so tight that "1.6 MB" runs into the plot.
  const PAD = useMemo(
    () => ({ top: 12, right: 12, bottom: 22, left: narrow ? 44 : 52 }),
    [narrow],
  );

  const model = useMemo(() => {
    if (samples.length < 2) return null;

    const t0 = samples[0]!.at;
    const t1 = samples[samples.length - 1]!.at;
    const span = Math.max(1, t1 - t0);
    const peak = Math.max(
      ...samples.map((s) => s.live_bytes + s.archive_bytes),
      1,
    );

    const plotW = W - PAD.left - PAD.right;
    const plotH = H - PAD.top - PAD.bottom;
    const x = (at: number) => PAD.left + ((at - t0) / span) * plotW;
    const y = (bytes: number) => PAD.top + plotH - (bytes / peak) * plotH;

    const points: Point[] = samples.map((s) => ({
      x: x(s.at),
      live: s.live_bytes,
      archive: s.archive_bytes,
      at: s.at,
    }));

    // Archived sits on the baseline and live stacks on top: the archive is the
    // stable floor, and live is the part that moves day to day.
    const archiveCurve = points.map((p) => [p.x, y(p.archive)] as const);
    const totalCurve = points.map((p) => [p.x, y(p.archive + p.live)] as const);

    const archiveArea = area(archiveCurve, y(0));
    // The live band is the region *between* the two curves, not from the total
    // down to the baseline. Filling to the baseline would paint a second layer
    // over the archived band, and the two translucent fills would blend into a
    // third colour that means nothing.
    const liveBand = band(totalCurve, archiveCurve);
    const totalLine = line(totalCurve);
    const archiveLine = line(archiveCurve);

    return { points, peak, y, archiveArea, liveBand, totalLine, archiveLine, t0, t1 };
  }, [samples, W, H, PAD]);

  if (!model) {
    return (
      <p ref={boxRef as React.RefObject<HTMLParagraphElement>} className="py-8 text-center text-sm text-fg-dim">
        Not enough history yet. Usage is recorded every half hour, so the trend
        appears after the first day.
      </p>
    );
  }

  const active = hover !== null ? model.points[hover] : null;

  const onMove = (e: React.PointerEvent<SVGSVGElement>) => {
    const rect = svgRef.current?.getBoundingClientRect();
    if (!rect) return;
    const px = ((e.clientX - rect.left) / rect.width) * W;
    // Nearest sample rather than the one under the cursor, so the crosshair
    // never sits between two points showing neither.
    let best = 0;
    let bestDist = Infinity;
    for (let i = 0; i < model.points.length; i++) {
      const d = Math.abs(model.points[i]!.x - px);
      if (d < bestDist) {
        bestDist = d;
        best = i;
      }
    }
    setHover(best);
  };

  // Four gridlines crowd a 150px-tall plot into stripes; two are enough to
  // read a value off.
  const gridValues = (narrow ? [0.5, 1] : [0.25, 0.5, 0.75, 1]).map((f) => f * model.peak);

  return (
    <figure ref={boxRef as React.RefObject<HTMLElement>} className="m-0">
      <svg
        ref={svgRef}
        viewBox={`0 0 ${W} ${H}`}
        className="w-full touch-none"
        role="img"
        aria-label="Footage held over time, split into live and archived"
        onPointerMove={onMove}
        onPointerLeave={() => setHover(null)}
      >
        {/* Grid and axis stay recessive — they orient, they don't compete. */}
        {gridValues.map((v) => (
          <g key={v}>
            <line
              x1={PAD.left}
              x2={W - PAD.right}
              y1={model.y(v)}
              y2={model.y(v)}
              stroke="var(--line)"
              strokeWidth="1"
            />
            <text
              x={PAD.left - 8}
              y={model.y(v) + 3.5}
              textAnchor="end"
              className="fill-[var(--fg-faint)] font-mono text-[10px]"
            >
              {formatBytes(v, 1)}
            </text>
          </g>
        ))}

        <path d={model.archiveArea} fill="var(--chart-archive)" fillOpacity="0.28" />
        {/* A 2px gap in the surface colour separates the stacked fills, so the
            boundary reads as a boundary rather than a colour change. */}
        <path
          d={model.archiveLine}
          fill="none"
          stroke="var(--panel)"
          strokeWidth="3"
          strokeLinejoin="round"
        />
        <path d={model.liveBand} fill="var(--chart-live)" fillOpacity="0.22" />
        <path
          d={model.totalLine}
          fill="none"
          stroke="var(--chart-live)"
          strokeWidth="2"
          strokeLinejoin="round"
          strokeLinecap="round"
        />
        <path
          d={model.archiveLine}
          fill="none"
          stroke="var(--chart-archive)"
          strokeWidth="2"
          strokeLinejoin="round"
          strokeLinecap="round"
        />

        {active && (
          <g>
            <line
              x1={active.x}
              x2={active.x}
              y1={PAD.top}
              y2={H - PAD.bottom}
              stroke="var(--line-bright)"
              strokeWidth="1"
            />
            {/* A surface-coloured ring keeps the markers legible where the two
                series overlap. */}
            <circle
              cx={active.x}
              cy={model.y(active.archive + active.live)}
              r="4"
              fill="var(--chart-live)"
              stroke="var(--panel)"
              strokeWidth="2"
            />
            <circle
              cx={active.x}
              cy={model.y(active.archive)}
              r="4"
              fill="var(--chart-archive)"
              stroke="var(--panel)"
              strokeWidth="2"
            />
          </g>
        )}

        <text
          x={PAD.left}
          y={H - 6}
          className="fill-[var(--fg-faint)] font-mono text-[10px]"
        >
          {shortDate(model.t0)}
        </text>
        <text
          x={W - PAD.right}
          y={H - 6}
          textAnchor="end"
          className="fill-[var(--fg-faint)] font-mono text-[10px]"
        >
          {shortDate(model.t1)}
        </text>
      </svg>

      <figcaption className="mt-2 flex flex-wrap items-center gap-4">
        <Key color="var(--chart-live)" label="Live" value={formatBytes(last(samples).live_bytes)} />
        <Key
          color="var(--chart-archive)"
          label="Archived"
          value={formatBytes(last(samples).archive_bytes)}
        />
        {active && (
          <span className="data ml-auto text-[11px] text-fg-dim">
            {new Date(active.at * 1000).toLocaleString(undefined, {
              month: "short",
              day: "2-digit",
              hour: "2-digit",
              minute: "2-digit",
              hourCycle: "h23",
            })}
            {" · "}
            {formatBytes(active.live)} live + {formatBytes(active.archive)} archived
          </span>
        )}
      </figcaption>
    </figure>
  );
}

/** Identity is a swatch plus a text label, never colour alone. */
function Key({ color, label, value }: { color: string; label: string; value: string }) {
  return (
    <span className="flex items-center gap-2">
      <span
        className="h-2 w-2 rounded-[1px]"
        style={{ background: color }}
        aria-hidden
      />
      <span className="text-xs text-fg-dim">{label}</span>
      <span className="data text-[11px] text-fg">{value}</span>
    </span>
  );
}

function line(pts: readonly (readonly [number, number])[]) {
  return pts.map(([x, y], i) => `${i === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`).join(" ");
}

/** The closed region between an upper and a lower curve. */
function band(
  upper: readonly (readonly [number, number])[],
  lower: readonly (readonly [number, number])[],
) {
  if (upper.length === 0) return "";
  const back = [...lower]
    .reverse()
    .map(([x, y]) => `L${x.toFixed(1)},${y.toFixed(1)}`)
    .join(" ");
  return `${line(upper)} ${back} Z`;
}

function area(pts: readonly (readonly [number, number])[], baseline: number) {
  if (pts.length === 0) return "";
  const first = pts[0]!;
  const last = pts[pts.length - 1]!;
  return `${line(pts)} L${last[0].toFixed(1)},${baseline.toFixed(1)} L${first[0].toFixed(1)},${baseline.toFixed(1)} Z`;
}

function shortDate(at: number) {
  return new Date(at * 1000).toLocaleDateString(undefined, {
    month: "short",
    day: "2-digit",
  });
}

function last(samples: StorageSample[]) {
  return samples[samples.length - 1]!;
}
