import { Plus } from "lucide-react";
import type { AppSnapshot } from "../lib/types";
import { AccountCard } from "../components/AccountCard";

interface Props {
  snapshot: AppSnapshot;
  busyId: string | null;
  onAdd: () => void;
  onActivate: (id: string) => void;
  onRename: (id: string, label: string) => void;
  onRemove: (id: string) => void;
}

export function Dashboard(props: Props) {
  const { snapshot } = props;

  return (
    <div className="flex flex-1 flex-col gap-3 overflow-y-auto px-4 pb-4">
      <div className="flex items-center justify-between px-1 pt-1">
        <span className="text-[11px] font-medium uppercase tracking-wide text-[var(--color-ink-faint)]">
          Accounts · {snapshot.accounts.length}
        </span>
        <button
          onClick={props.onAdd}
          className="no-drag flex items-center gap-1 rounded-lg px-2 py-1 text-xs font-medium text-[var(--color-clay-soft)] hover:bg-white/5"
        >
          <Plus size={14} /> Add
        </button>
      </div>

      <div className="flex flex-col gap-2.5">
        {snapshot.accounts.map((account) => (
          <AccountCard
            key={account.id}
            account={account}
            busy={props.busyId === account.id}
            onActivate={props.onActivate}
            onRename={props.onRename}
            onRemove={props.onRemove}
          />
        ))}
      </div>

      <p className="px-1 pt-0.5 text-[11px] leading-relaxed text-[var(--color-ink-faint)]">
        Switching rewrites Claude Code's login in place. A running session keeps
        its old account until it exits — to pick up the new one without losing
        context, start <code>claude -c</code> (continue) in your terminal.
      </p>
    </div>
  );
}
