import { Zap, Pin } from "lucide-react";
import type { AppSnapshot } from "../lib/types";

interface Props {
  snapshot: AppSnapshot;
  onAuto: () => void;
}

/** Shows the current routing mode with a one-tap return to Auto. */
export function RoutingBar({ snapshot, onAuto }: Props) {
  const mode = snapshot.mode;
  const auto = mode.kind === "auto";
  const pinnedLabel =
    mode.kind === "pinned"
      ? snapshot.accounts.find((a) => a.id === mode.accountId)?.label
      : undefined;

  return (
    <div className="card no-drag flex items-center justify-between px-3.5 py-2.5">
      <div className="flex items-center gap-2.5">
        <div
          className="grid h-7 w-7 place-items-center rounded-lg"
          style={{
            background: auto
              ? "rgba(217,119,87,0.15)"
              : "var(--color-surface-2)",
            color: auto ? "var(--color-clay)" : "var(--color-ink-soft)",
          }}
        >
          {auto ? <Zap size={15} /> : <Pin size={15} />}
        </div>
        <div className="leading-tight">
          <div className="text-xs font-medium">
            {auto ? "Auto — balance & fail over" : `Pinned to ${pinnedLabel ?? "account"}`}
          </div>
          <div className="text-[11px] text-[var(--color-ink-faint)]">
            {auto
              ? "Switches before you hit a limit"
              : "Always uses this account · no failover"}
          </div>
        </div>
      </div>

      {!auto && (
        <button
          onClick={onAuto}
          className="no-drag rounded-lg bg-[var(--color-surface-2)] px-2.5 py-1 text-xs font-medium text-[var(--color-ink-soft)] hover:bg-white/5"
        >
          Switch to Auto
        </button>
      )}
    </div>
  );
}
