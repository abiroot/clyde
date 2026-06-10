import { pct, pressureColor } from "../lib/format";

interface Props {
  label: string;
  value: number | null;
}

/** A slim labelled utilization bar that shifts green → amber → red. */
export function UsageGauge({ label, value }: Props) {
  const p = pct(value);
  const color = pressureColor(p);
  const unknown = value == null;

  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-baseline justify-between text-[11px]">
        <span className="text-[var(--color-ink-faint)] uppercase tracking-wide">
          {label}
        </span>
        <span
          className="font-medium tabular-nums"
          style={{ color: unknown ? "var(--color-ink-faint)" : color }}
        >
          {unknown ? "—" : `${Math.round(p)}%`}
        </span>
      </div>
      <div className="h-1.5 w-full rounded-full bg-[var(--color-surface-2)] overflow-hidden">
        <div
          className="h-full rounded-full transition-[width] duration-500 ease-out"
          style={{
            width: `${unknown ? 0 : p}%`,
            background: color,
            boxShadow: unknown ? "none" : `0 0 8px ${color}55`,
          }}
        />
      </div>
    </div>
  );
}
