/** Color for a utilization percentage. */
export function pressureColor(pct: number): string {
  if (pct >= 90) return "var(--color-danger)";
  if (pct >= 70) return "var(--color-warn)";
  return "var(--color-ok)";
}

/** "resets in 2h 14m" from a unix-seconds timestamp. */
export function resetsIn(epochSeconds: number | null): string | null {
  if (!epochSeconds) return null;
  const ms = epochSeconds * 1000 - Date.now();
  if (ms <= 0) return "resetting…";
  const mins = Math.round(ms / 60000);
  if (mins < 60) return `resets in ${mins}m`;
  const h = Math.floor(mins / 60);
  const m = mins % 60;
  return `resets in ${h}h ${m}m`;
}

export function pct(value: number | null): number {
  if (value == null) return 0;
  return Math.max(0, Math.min(100, value));
}
