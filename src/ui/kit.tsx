import type { ReactNode } from "react";
import type { AccountView, Forecast } from "../lib/types";

/*
 * Native-style building blocks shared by the popover and the main window.
 * Colours come from the `--n-*` tokens in native.css, so everything follows
 * the system light/dark setting.
 */

// ---- limits ----------------------------------------------------------------

export interface LimitRowData {
  label: string;
  percent: number | null;
  resetsAt: number | null;
  severity?: string | null;
  forecast?: Forecast | null;
}

/** An account's limits in display order: session, week, then per-model caps. */
export function limitRows(a: AccountView): LimitRowData[] {
  const u = a.usage;
  const fc = (label: string) => a.forecasts?.find((f) => f.label === label) ?? null;
  return [
    { label: "Session", percent: u.five_hour_utilization, resetsAt: u.five_hour_resets_at ?? null, forecast: fc("Session") },
    { label: "Week", percent: u.seven_day_utilization, resetsAt: u.seven_day_resets_at ?? null, forecast: fc("Week") },
    ...(u.scoped_limits ?? []).map((l) => ({
      label: scopedLabel(l.label),
      percent: l.percent,
      resetsAt: l.resets_at,
      severity: l.severity,
      forecast: fc(l.label),
    })),
  ];
}

export function LimitRow({ label, percent, resetsAt, severity, forecast, large = false }: LimitRowData & { large?: boolean }) {
  const p = percent == null ? null : clamp(percent);
  const color = toneColor(p, severity);
  const eta = forecast?.minutes_to_full;
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-baseline gap-2">
        <span className={`flex-1 ${large ? "text-[13px]" : ""}`}>{label}</span>
        {resetsAt && <span className="n-dim text-[11px]">{shortReset(resetsAt)}</span>}
        <span
          className={`text-right font-medium tabular-nums ${large ? "w-12 text-[15px]" : "w-9"}`}
          style={{ color: p == null ? "var(--n-text-3)" : color }}
        >
          {p == null ? "—" : `${Math.round(p)}%`}
        </span>
      </div>
      <div className="n-bar" style={large ? { height: 6, borderRadius: 3 } : undefined}>
        <span style={{ width: `${p ?? 0}%`, background: color }} />
      </div>
      {eta != null && eta < 7 * 24 * 60 && p != null && p < 100 && (
        <span className="text-[11px]" style={{ color: eta < 180 ? "var(--n-warn)" : "var(--n-text-2)" }}>
          At this pace, full in {duration(eta)}
        </span>
      )}
    </div>
  );
}

/** The account's saved login no longer works — switching to it would hand
 *  Claude Code a dead credential. */
export function loginBroken(a: AccountView): boolean {
  return !!a.usage_error && /refresh|Signed out/i.test(a.usage_error);
}

/** The highest percentage across an account's limits. */
export function tightest(a: AccountView): number | null {
  const u = a.usage;
  const all = [u.five_hour_utilization, u.seven_day_utilization, ...(u.scoped_limits ?? []).map((l) => l.percent)]
    .filter((v): v is number => v != null);
  return all.length ? Math.max(...all) : null;
}

/** One-line headroom summary for a non-active account. */
export function AccountSummary({ account }: { account: AccountView }) {
  const u = account.usage;
  if (account.usage_error)
    return <span style={{ color: "var(--n-warn)" }}>{account.usage_error}</span>;
  if (u.status === "rejected")
    return (
      <span style={{ color: "var(--n-danger)" }}>
        Limit reached{u.resets_at ? ` · ${shortReset(u.resets_at)}` : ""}
      </span>
    );
  if (u.updated_at === 0) return <span className="n-dim">No usage read yet</span>;
  const scoped = (u.scoped_limits ?? []).filter((l) => l.percent >= 50);
  return (
    <span className="n-dim">
      Session {fmt(u.five_hour_utilization)} · Week {fmt(u.seven_day_utilization)}
      {scoped.map((l) => ` · ${scopedLabel(l.label).split(" · ")[0]} ${Math.round(l.percent)}%`).join("")}
    </span>
  );
}

// ---- layout ----------------------------------------------------------------

/** A System Settings–style group: optional title, rounded box, divided rows. */
export function Group({ title, footer, children }: { title?: string; footer?: ReactNode; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-1.5">
      {title && <h3 className="px-1 text-[12px] font-semibold text-[var(--n-text-2)]">{title}</h3>}
      <div className="n-group">{children}</div>
      {footer && <p className="px-1 text-[11px] leading-relaxed text-[var(--n-text-2)]">{footer}</p>}
    </section>
  );
}

export function GroupRow({ children, className = "" }: { children: ReactNode; className?: string }) {
  return <div className={`n-group-row ${className}`}>{children}</div>;
}

export function Toggle({ checked, onChange, label }: { checked: boolean; onChange: (v: boolean) => void; label: string }) {
  return (
    <button
      role="switch"
      aria-checked={checked}
      aria-label={label}
      onClick={() => onChange(!checked)}
      className="n-switch"
      data-on={checked}
    >
      <span />
    </button>
  );
}

export function Segmented<T extends string>({
  value,
  options,
  onChange,
}: {
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
}) {
  return (
    <div className="n-segmented" role="tablist">
      {options.map((o) => (
        <button key={o.value} role="tab" aria-selected={o.value === value} onClick={() => onChange(o.value)}>
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function Avatar({ label, active = false, size = 24 }: { label: string; active?: boolean; size?: number }) {
  return (
    <span
      className="grid shrink-0 place-items-center rounded-full font-semibold"
      style={{
        width: size,
        height: size,
        fontSize: Math.round(size * 0.45),
        background: active ? "var(--n-accent)" : "var(--n-track)",
        color: active ? "var(--n-on-accent)" : "var(--n-text-2)",
      }}
    >
      {(label || "?").trim().charAt(0).toUpperCase()}
    </span>
  );
}

// ---- formatting ------------------------------------------------------------

export function clamp(v: number): number {
  return Math.max(0, Math.min(100, v));
}

/** Anthropic's own severity when it gives one; otherwise our thresholds. */
export function toneColor(p: number | null, severity?: string | null): string {
  if (severity === "critical") return "var(--n-danger)";
  if (severity === "warning") return "var(--n-warn)";
  if (p == null) return "var(--n-text-3)";
  if (p >= 90) return "var(--n-danger)";
  if (p >= 75) return "var(--n-warn)";
  return "var(--n-ok)";
}

/** "7-day · Fable" → "Fable · week". */
export function scopedLabel(label: string): string {
  const [window, name] = label.split(" · ");
  if (!name) return label;
  const w = window === "7-day" ? "week" : window === "5-hour" ? "session" : window;
  return `${name} · ${w}`;
}

export function fmt(v: number | null): string {
  return v == null ? "—" : `${Math.round(v)}%`;
}

export function duration(mins: number): string {
  if (mins < 60) return `${Math.max(1, Math.round(mins))}m`;
  const h = Math.floor(mins / 60);
  if (h < 24) return `${h}h ${Math.round(mins % 60)}m`;
  return `${Math.floor(h / 24)}d ${h % 24}h`;
}

export function shortReset(epochSeconds: number): string {
  const mins = Math.round((epochSeconds * 1000 - Date.now()) / 60000);
  return mins <= 0 ? "resetting" : `resets in ${duration(mins)}`;
}

export function ago(ms: number): string {
  if (!ms) return "never";
  const s = Math.round((Date.now() - ms) / 1000);
  if (s < 60) return "just now";
  const m = Math.round(s / 60);
  return m < 60 ? `${m}m ago` : `${Math.round(m / 60)}h ago`;
}
