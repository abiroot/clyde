import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../lib/api";
import type { AppSnapshot, HistoryPoint } from "../lib/types";
import { Group, GroupRow, Segmented, duration, scopedLabel } from "../ui/kit";

type Range = "24" | "168" | "720";
const RANGES: { value: Range; label: string }[] = [
  { value: "24", label: "24 hours" },
  { value: "168", label: "7 days" },
  { value: "720", label: "30 days" },
];

/** Series colours in fixed order (dataviz reference palette, validated). */
const SERIES = ["var(--n-series-1)", "var(--n-series-2)", "var(--n-series-3)", "var(--n-series-4)"];

/** History label → display label ("7-day · Fable" → "Fable · week"). */
const display = (k: string) => (k.includes(" · ") ? scopedLabel(k) : k);

export function UsagePage({ snapshot }: { snapshot: AppSnapshot }) {
  const [accountId, setAccountId] = useState(snapshot.active_id ?? snapshot.accounts[0]?.id ?? "");
  const [range, setRange] = useState<Range>("24");
  const [points, setPoints] = useState<HistoryPoint[]>([]);

  useEffect(() => {
    if (!accountId) return;
    let live = true;
    const load = () => api.getHistory(accountId, Number(range)).then((p) => live && setPoints(p));
    load();
    const t = setInterval(load, 60_000);
    return () => {
      live = false;
      clearInterval(t);
    };
  }, [accountId, range, snapshot]);

  const account = snapshot.accounts.find((a) => a.id === accountId);
  // Series keys in a stable order: Session, Week, then scoped caps by name.
  const keys = useMemo(() => {
    const all = new Set<string>();
    points.forEach((p) => Object.keys(p.limits).forEach((k) => all.add(k)));
    const scoped = [...all].filter((k) => k !== "Session" && k !== "Week").sort();
    return [...["Session", "Week"].filter((k) => all.has(k)), ...scoped].slice(0, SERIES.length);
  }, [points]);

  if (snapshot.accounts.length === 0) return <p className="n-dim">Add an account to see its usage.</p>;

  return (
    <div className="flex max-w-[760px] flex-col gap-6">
      <div className="flex flex-wrap items-center gap-3">
        <select
          value={accountId}
          onChange={(e) => setAccountId(e.target.value)}
          className="rounded-md border border-[var(--n-card-border)] bg-[var(--n-window)] px-2 py-1 text-[13px]"
        >
          {snapshot.accounts.map((a) => (
            <option key={a.id} value={a.id}>
              {a.label}
              {a.is_active ? " (active)" : ""}
            </option>
          ))}
        </select>
        <Segmented value={range} options={RANGES} onChange={setRange} />
      </div>

      <Group>
        <div className="p-4">
          {points.length < 2 ? (
            <p className="n-dim py-10 text-center">
              History builds up as Clyde checks usage (every 2 minutes). Come back in a little while.
            </p>
          ) : (
            <>
              <Chart points={points} keys={keys} hours={Number(range)} />
              {points[0].t > Date.now() - Number(range) * 3_600_000 + 3_600_000 && (
                <p className="n-dim mt-1 text-[11px]">
                  Collecting since{" "}
                  {new Date(points[0].t).toLocaleString([], { weekday: "short", hour: "2-digit", minute: "2-digit" })}
                </p>
              )}
            </>
          )}
        </div>
      </Group>

      <Group title="Now and pace" footer="Pace is the trend over the last 3 hours since the latest reset.">
        {keys.length === 0 && (
          <GroupRow>
            <span className="n-dim">No data yet.</span>
          </GroupRow>
        )}
        {keys.map((k, i) => {
          const values = points.map((p) => p.limits[k]).filter((v): v is number => v != null);
          const now = values[values.length - 1];
          const peak = values.length ? Math.max(...values) : null;
          const f = account?.forecasts.find((x) => x.label === k);
          return (
            <GroupRow key={k}>
              <span className="h-2.5 w-2.5 shrink-0 rounded-full" style={{ background: SERIES[i] }} aria-hidden />
              <span className="flex-1">{display(k)}</span>
              <span className="n-dim w-24 text-right text-[12px]">peak {peak == null ? "—" : `${Math.round(peak)}%`}</span>
              <span className="n-dim w-40 text-right text-[12px]">
                {f?.minutes_to_full != null
                  ? `full in ${duration(f.minutes_to_full)}`
                  : f
                    ? `${f.per_hour > 0 ? "+" : ""}${f.per_hour}%/h`
                    : "—"}
              </span>
              <span className="w-12 text-right font-medium tabular-nums">{now == null ? "—" : `${Math.round(now)}%`}</span>
            </GroupRow>
          );
        })}
      </Group>
    </div>
  );
}

// ---- chart -----------------------------------------------------------------

const H = 220;
const PAD = { top: 12, right: 92, bottom: 22, left: 34 };

function Chart({ points, keys, hours }: { points: HistoryPoint[]; keys: string[]; hours: number }) {
  const wrap = useRef<HTMLDivElement>(null);
  const [w, setW] = useState(600);
  const [hover, setHover] = useState<number | null>(null);

  useEffect(() => {
    const el = wrap.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setW(el.clientWidth));
    ro.observe(el);
    setW(el.clientWidth);
    return () => ro.disconnect();
  }, []);

  const t1 = Date.now();
  const t0 = t1 - hours * 3_600_000;
  const iw = Math.max(50, w - PAD.left - PAD.right);
  const ih = H - PAD.top - PAD.bottom;
  const x = (t: number) => PAD.left + ((t - t0) / (t1 - t0)) * iw;
  const y = (v: number) => PAD.top + (1 - Math.max(0, Math.min(100, v)) / 100) * ih;

  const lines = keys.map((k) => {
    const pts = points.filter((p) => p.limits[k] != null);
    return { k, pts, d: pts.map((p, i) => `${i ? "L" : "M"}${x(p.t).toFixed(1)},${y(p.limits[k]).toFixed(1)}`).join("") };
  });

  // Direct end labels, nudged apart so they never collide.
  const ends = lines
    .filter((l) => l.pts.length)
    .map((l, i) => ({ k: l.k, i, v: l.pts[l.pts.length - 1].limits[l.k], y: y(l.pts[l.pts.length - 1].limits[l.k]) }))
    .sort((a, b) => a.y - b.y);
  for (let i = 1; i < ends.length; i++) ends[i].y = Math.max(ends[i].y, ends[i - 1].y + 13);

  const hp = hover == null ? null : points[hover];
  const ticks = timeTicks(t0, t1, hours);

  return (
    <div ref={wrap} className="relative">
      {/* Legend */}
      <div className="mb-2 flex flex-wrap gap-x-4 gap-y-1 text-[12px] text-[var(--n-text-2)]">
        {keys.map((k, i) => (
          <span key={k} className="flex items-center gap-1.5">
            <span className="h-[2px] w-3.5 rounded" style={{ background: SERIES[i] }} />
            {display(k)}
          </span>
        ))}
      </div>

      <svg
        width={w}
        height={H}
        role="img"
        aria-label={`Usage over the last ${hours} hours`}
        onMouseMove={(e) => {
          const r = (e.currentTarget as SVGSVGElement).getBoundingClientRect();
          const t = t0 + ((e.clientX - r.left - PAD.left) / iw) * (t1 - t0);
          let best = 0;
          points.forEach((p, i) => {
            if (Math.abs(p.t - t) < Math.abs(points[best].t - t)) best = i;
          });
          setHover(best);
        }}
        onMouseLeave={() => setHover(null)}
      >
        {/* Recessive grid: 0 / 50 / 100 */}
        {[0, 50, 100].map((v) => (
          <g key={v}>
            <line x1={PAD.left} x2={PAD.left + iw} y1={y(v)} y2={y(v)} stroke="var(--n-sep)" />
            <text x={PAD.left - 6} y={y(v) + 3.5} textAnchor="end" fontSize="10" fill="var(--n-text-3)">
              {v}%
            </text>
          </g>
        ))}
        {ticks.map((t) => (
          <text key={t.t} x={x(t.t)} y={H - 6} textAnchor="middle" fontSize="10" fill="var(--n-text-3)">
            {t.label}
          </text>
        ))}

        {lines.map((l, i) => (
          <path key={l.k} d={l.d} fill="none" stroke={SERIES[i]} strokeWidth={2} strokeLinejoin="round" strokeLinecap="round" />
        ))}

        {ends.map((e) => (
          <text key={e.k} x={PAD.left + iw + 8} y={e.y + 3.5} fontSize="11" fill="var(--n-text-2)">
            {display(e.k)} {Math.round(e.v)}%
          </text>
        ))}

        {hp && (
          <g pointerEvents="none">
            <line x1={x(hp.t)} x2={x(hp.t)} y1={PAD.top} y2={PAD.top + ih} stroke="var(--n-text-3)" />
            {keys.map((k, i) =>
              hp.limits[k] == null ? null : (
                <circle key={k} cx={x(hp.t)} cy={y(hp.limits[k])} r={4} fill={SERIES[i]} stroke="var(--n-window)" strokeWidth={2} />
              ),
            )}
          </g>
        )}
      </svg>

      {hp && (
        <div
          className="card pointer-events-none absolute z-10 px-2.5 py-1.5 text-[11px]"
          style={{ left: Math.min(x(hp.t) + 10, w - 170), top: 28 }}
        >
          <div className="mb-0.5 font-medium">{new Date(hp.t).toLocaleString([], { weekday: "short", hour: "2-digit", minute: "2-digit" })}</div>
          {keys.map((k, i) =>
            hp.limits[k] == null ? null : (
              <div key={k} className="flex items-center gap-1.5">
                <span className="h-2 w-2 rounded-full" style={{ background: SERIES[i] }} />
                <span className="flex-1 text-[var(--n-text-2)]">{display(k)}</span>
                <span className="tabular-nums">{Math.round(hp.limits[k])}%</span>
              </div>
            ),
          )}
        </div>
      )}
    </div>
  );
}

function timeTicks(t0: number, t1: number, hours: number): { t: number; label: string }[] {
  const step = hours <= 24 ? 6 * 3_600_000 : hours <= 168 ? 24 * 3_600_000 : 5 * 24 * 3_600_000;
  const out: { t: number; label: string }[] = [];
  const start = Math.ceil(t0 / step) * step;
  for (let t = start; t <= t1; t += step) {
    const d = new Date(t);
    out.push({
      t,
      label:
        hours <= 24
          ? d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
          : d.toLocaleDateString([], { month: "short", day: "numeric" }),
    });
  }
  return out;
}
