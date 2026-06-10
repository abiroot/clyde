import { useState } from "react";
import { Check, CheckCircle2, Pencil, Trash2, X } from "lucide-react";
import type { AccountView } from "../lib/types";
import { UsageGauge } from "./UsageGauge";
import { resetsIn } from "../lib/format";

interface Props {
  account: AccountView;
  busy: boolean;
  onActivate: (id: string) => void;
  onRename: (id: string, label: string) => void;
  onRemove: (id: string) => void;
}

export function AccountCard({ account, busy, onActivate, onRename, onRemove }: Props) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(account.label);
  const [confirmDelete, setConfirmDelete] = useState(false);

  const limited = account.usage.status === "rejected";
  const resets = resetsIn(account.usage.resets_at);
  const initial = (account.label || "?").trim().charAt(0).toUpperCase();

  return (
    <div
      className={`card no-drag fade-in p-4 flex flex-col gap-3.5 transition-colors ${
        account.is_active ? "ring-1 ring-[var(--color-clay)]/40" : ""
      }`}
    >
      {/* Header */}
      <div className="flex items-center gap-3">
        <div
          className="grid h-9 w-9 shrink-0 place-items-center rounded-xl text-sm font-semibold"
          style={{
            background: account.is_active
              ? "var(--color-clay)"
              : "var(--color-surface-2)",
            color: account.is_active ? "#1a0f0a" : "var(--color-ink-soft)",
          }}
        >
          {initial}
        </div>

        <div className="min-w-0 flex-1">
          {editing ? (
            <div className="flex items-center gap-1.5">
              <input
                autoFocus
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    onRename(account.id, draft.trim() || account.label);
                    setEditing(false);
                  }
                  if (e.key === "Escape") setEditing(false);
                }}
                className="w-full rounded-lg bg-[var(--color-surface-2)] px-2 py-1 text-sm outline-none focus:ring-1 focus:ring-[var(--color-clay)]/50"
              />
              <button
                className="rounded-md p-1 text-[var(--color-ok)] hover:bg-white/5"
                onClick={() => {
                  onRename(account.id, draft.trim() || account.label);
                  setEditing(false);
                }}
              >
                <Check size={15} />
              </button>
              <button
                className="rounded-md p-1 text-[var(--color-ink-faint)] hover:bg-white/5"
                onClick={() => setEditing(false)}
              >
                <X size={15} />
              </button>
            </div>
          ) : (
            <>
              <div className="flex items-center gap-2">
                <span className="truncate text-sm font-medium">{account.label}</span>
                {account.is_active && (
                  <span className="rounded-full bg-[var(--color-clay)]/15 px-1.5 py-0.5 text-[10px] font-medium text-[var(--color-clay-soft)]">
                    active
                  </span>
                )}
              </div>
              <div className="truncate text-xs text-[var(--color-ink-faint)]">
                {account.email ?? "Claude account"}
                {account.subscription_type ? ` · ${account.subscription_type}` : ""}
              </div>
            </>
          )}
        </div>

        {!editing &&
          (account.is_active ? (
            <span
              title="Active in Claude Code"
              className="no-drag flex items-center gap-1 rounded-lg px-1.5 py-1 text-[var(--color-ok)]"
            >
              <CheckCircle2 size={16} />
            </span>
          ) : (
            <button
              title="Make this the account Claude Code uses"
              disabled={busy}
              onClick={() => onActivate(account.id)}
              className="no-drag rounded-lg bg-[var(--color-clay)]/10 px-2.5 py-1 text-xs font-medium text-[var(--color-clay-soft)] transition-colors hover:bg-[var(--color-clay)]/20 disabled:opacity-50"
            >
              {busy ? "Switching…" : "Use"}
            </button>
          ))}
      </div>

      {/* Gauges */}
      <div className="grid grid-cols-2 gap-3">
        <UsageGauge label="5-hour" value={account.usage.five_hour_utilization} />
        <UsageGauge label="7-day" value={account.usage.seven_day_utilization} />
      </div>

      {/* Footer */}
      <div className="flex items-center justify-between">
        <span className="text-[11px] text-[var(--color-ink-faint)]">
          {limited ? (
            <span className="text-[var(--color-danger)]">
              limit reached{resets ? ` · ${resets}` : ""}
            </span>
          ) : resets ? (
            resets
          ) : (
            "ready"
          )}
        </span>

        {confirmDelete ? (
          <div className="flex items-center gap-1.5 text-[11px]">
            <span className="text-[var(--color-ink-faint)]">Remove?</span>
            <button
              className="rounded-md px-1.5 py-0.5 text-[var(--color-danger)] hover:bg-[var(--color-danger)]/10"
              onClick={() => onRemove(account.id)}
            >
              Yes
            </button>
            <button
              className="rounded-md px-1.5 py-0.5 text-[var(--color-ink-soft)] hover:bg-white/5"
              onClick={() => setConfirmDelete(false)}
            >
              No
            </button>
          </div>
        ) : (
          <div className="flex items-center gap-0.5">
            <button
              title="Rename"
              onClick={() => {
                setDraft(account.label);
                setEditing(true);
              }}
              className="rounded-md p-1 text-[var(--color-ink-faint)] hover:bg-white/5 hover:text-[var(--color-ink-soft)]"
            >
              <Pencil size={14} />
            </button>
            <button
              title="Remove account"
              onClick={() => setConfirmDelete(true)}
              className="rounded-md p-1 text-[var(--color-ink-faint)] hover:bg-[var(--color-danger)]/10 hover:text-[var(--color-danger)]"
            >
              <Trash2 size={14} />
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
