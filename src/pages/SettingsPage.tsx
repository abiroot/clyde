import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { api } from "../lib/api";
import type { Settings } from "../lib/types";
import { Group, GroupRow, Toggle } from "../ui/kit";

const THRESHOLDS = [50, 75, 80, 90, 95, 100];

const SHORTCUTS: { value: string; label: string }[] = [
  { value: "Alt+Super+C", label: "⌥⌘C" },
  { value: "Alt+Super+K", label: "⌥⌘K" },
  { value: "Control+Alt+C", label: "⌃⌥C" },
  { value: "", label: "None" },
];

export function SettingsPage() {
  const [s, setS] = useState<Settings | null>(null);
  const [autostart, setAutostart] = useState(false);
  const [version, setVersion] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.getSettings().then(setS);
    api.getAutostart().then(setAutostart).catch(() => {});
    getVersion().then(setVersion).catch(() => {});
  }, []);

  const save = async (patch: Partial<Settings>) => {
    if (!s) return;
    const next = { ...s, ...patch };
    setS(next);
    try {
      setS(await api.setSettings(next));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  };

  if (!s) return null;

  const toggleThreshold = (t: number) =>
    save({
      alert_thresholds: s.alert_thresholds.includes(t)
        ? s.alert_thresholds.filter((x) => x !== t)
        : [...s.alert_thresholds, t],
    });

  return (
    <div className="flex max-w-[600px] flex-col gap-6">
      <Group
        title="Alerts"
        footer="Clyde checks usage every 2 minutes and notifies once when a limit crosses a threshold on the way up — including per-model caps like Fable's weekly limit."
      >
        <GroupRow>
          <span className="flex-1">Notify me about limits</span>
          <Toggle label="Limit alerts" checked={s.alerts_enabled} onChange={(v) => save({ alerts_enabled: v })} />
        </GroupRow>
        {s.alerts_enabled && (
          <>
            <GroupRow>
              <span className="flex-1">At</span>
              <div className="flex flex-wrap justify-end gap-1">
                {THRESHOLDS.map((t) => {
                  const on = s.alert_thresholds.includes(t);
                  return (
                    <button
                      key={t}
                      onClick={() => toggleThreshold(t)}
                      aria-pressed={on}
                      className="rounded-md px-2 py-0.5 text-[12px] tabular-nums"
                      style={{
                        background: on ? "var(--n-accent)" : "var(--n-track)",
                        color: on ? "var(--n-on-accent)" : "var(--n-text)",
                      }}
                    >
                      {t}%
                    </button>
                  );
                })}
              </div>
            </GroupRow>
            <GroupRow>
              <span className="flex-1">When a well-used limit resets</span>
              <Toggle label="Reset alerts" checked={s.notify_on_reset} onChange={(v) => save({ notify_on_reset: v })} />
            </GroupRow>
            <GroupRow>
              <span className="flex-1">
                For every account
                <span className="n-dim block text-[11px]">Off: only the account Claude Code is using</span>
              </span>
              <Toggle
                label="Alerts for all accounts"
                checked={s.alert_all_accounts}
                onChange={(v) => save({ alert_all_accounts: v })}
              />
            </GroupRow>
            <GroupRow>
              <span className="flex-1 n-dim">macOS may ask to allow notifications the first time.</span>
              <button className="n-plain-button" onClick={() => api.testNotification()}>
                Send a test
              </button>
            </GroupRow>
          </>
        )}
      </Group>

      <Group title="Menu bar">
        <GroupRow>
          <span className="flex-1">
            Show usage next to the icon
            <span className="n-dim block text-[11px]">The active account's tightest limit, e.g. 94%</span>
          </span>
          <Toggle label="Menu bar readout" checked={s.menubar_readout} onChange={(v) => save({ menubar_readout: v })} />
        </GroupRow>
        <GroupRow>
          <span className="flex-1">Keyboard shortcut to open</span>
          <select
            value={s.shortcut}
            onChange={(e) => save({ shortcut: e.target.value })}
            className="rounded-md border border-[var(--n-card-border)] bg-[var(--n-window)] px-2 py-0.5 text-[13px]"
          >
            {SHORTCUTS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        </GroupRow>
      </Group>

      <Group title="General">
        <GroupRow>
          <span className="flex-1">Open Clyde at login</span>
          <Toggle
            label="Launch at login"
            checked={autostart}
            onChange={async (v) => {
              try {
                setAutostart(await api.setAutostart(v));
              } catch (e) {
                setError(String(e));
              }
            }}
          />
        </GroupRow>
      </Group>

      {error && (
        <p className="text-[12px]" style={{ color: "var(--n-danger)" }}>
          {error}
        </p>
      )}

      <Group title="About">
        <GroupRow>
          <span className="flex-1">Clyde</span>
          <span className="n-dim tabular-nums">{version}</span>
        </GroupRow>
        <GroupRow>
          <span className="n-dim text-[12px] leading-relaxed">
            For personal use. Anthropic's terms don't allow third-party apps to collect or store
            Claude.ai credentials, and Pro/Max limits assume ordinary, individual usage.
          </span>
        </GroupRow>
      </Group>
    </div>
  );
}
