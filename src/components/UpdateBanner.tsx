import { useState } from "react";
import { RefreshCw } from "lucide-react";
import { api } from "../lib/api";
import type { UpdateInfo } from "../lib/types";

/**
 * "Clyde 1.2.0 is available" with an Install button. Installing downloads the
 * signed release, verifies it, swaps the app and relaunches — so this is the
 * last thing the user sees of the old version.
 */
export function UpdateBanner({ update, compact = false }: { update: UpdateInfo; compact?: boolean }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const install = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.installUpdate();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return (
    <div
      className={`flex items-center gap-2.5 rounded-lg ${compact ? "px-2.5 py-1.5" : "px-3 py-2"}`}
      style={{ background: "color-mix(in srgb, var(--n-accent) 12%, transparent)" }}
    >
      <span className="min-w-0 flex-1 text-[12px]">
        <span className="font-medium">Clyde {update.version} is available</span>
        {!compact && <span className="n-dim"> · you have {update.current}</span>}
        {error && (
          <span className="block" style={{ color: "var(--n-danger)" }}>
            {error}
          </span>
        )}
      </span>
      <button className="n-button flex items-center gap-1.5" disabled={busy} onClick={install}>
        {busy && <RefreshCw size={11} className="animate-spin" />}
        {busy ? "Installing…" : "Install & restart"}
      </button>
    </div>
  );
}
