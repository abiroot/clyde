import { Link2, Link2Off, CheckCircle2, AlertCircle } from "lucide-react";
import type { AppSnapshot } from "../lib/types";

interface Props {
  snapshot: AppSnapshot;
  busy: boolean;
  onEnable: () => void;
  onDisable: () => void;
}

/** The "wire Clyde into Claude Code" control — the single switch that makes
 *  plain `claude` route through the proxy. */
export function IntegrationCard({ snapshot, busy, onEnable, onDisable }: Props) {
  const connected = snapshot.integration_enabled;
  const hasAccounts = snapshot.accounts.length > 0;

  if (connected) {
    return (
      <div className="card no-drag flex items-center justify-between px-3.5 py-3">
        <div className="flex items-center gap-2.5">
          <CheckCircle2 size={18} className="text-[var(--color-ok)]" />
          <div className="leading-tight">
            <div className="text-xs font-medium">Claude Code connected</div>
            <div className="text-[11px] text-[var(--color-ink-faint)]">
              Routing <code className="text-[var(--color-ink-soft)]">claude</code> through
              localhost:{snapshot.proxy_port}
              {snapshot.proxy_running ? "" : " · proxy offline"}
            </div>
          </div>
        </div>
        <button
          disabled={busy}
          onClick={onDisable}
          className="no-drag flex items-center gap-1.5 rounded-lg bg-[var(--color-surface-2)] px-2.5 py-1.5 text-xs font-medium text-[var(--color-ink-soft)] hover:bg-white/5 disabled:opacity-50"
        >
          <Link2Off size={14} /> Disconnect
        </button>
      </div>
    );
  }

  return (
    <div
      className="no-drag rounded-2xl p-4"
      style={{
        background:
          "linear-gradient(135deg, rgba(217,119,87,0.14), rgba(217,119,87,0.04))",
        border: "1px solid rgba(217,119,87,0.25)",
      }}
    >
      <div className="flex items-start gap-3">
        <Link2 size={18} className="mt-0.5 text-[var(--color-clay)]" />
        <div className="flex-1">
          <div className="text-sm font-medium">Connect Claude Code</div>
          <p className="mt-0.5 text-xs leading-relaxed text-[var(--color-ink-soft)]">
            Adds two keys to your <code>~/.claude/settings.json</code> so plain{" "}
            <code>claude</code> routes through Clyde. Your settings and history stay exactly
            where they are.
          </p>
          {!hasAccounts && (
            <p className="mt-2 flex items-center gap-1.5 text-[11px] text-[var(--color-warn)]">
              <AlertCircle size={13} /> Add an account first.
            </p>
          )}
          <button
            disabled={busy || !hasAccounts}
            onClick={onEnable}
            className="no-drag mt-3 w-full rounded-xl bg-[var(--color-clay)] px-3 py-2 text-sm font-semibold text-[#1a0f0a] transition-opacity hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-40"
          >
            {busy ? "Connecting…" : "Connect"}
          </button>
        </div>
      </div>
    </div>
  );
}
