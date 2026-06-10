import { Plus } from "lucide-react";
import type { AppSnapshot } from "../lib/types";
import { AccountCard } from "../components/AccountCard";
import { RoutingBar } from "../components/RoutingBar";
import { IntegrationCard } from "../components/IntegrationCard";

interface Props {
  snapshot: AppSnapshot;
  busy: boolean;
  onAdd: () => void;
  onPin: (id: string) => void;
  onAuto: () => void;
  onRename: (id: string, label: string) => void;
  onRemove: (id: string) => void;
  onEnable: () => void;
  onDisable: () => void;
}

export function Dashboard(props: Props) {
  const { snapshot } = props;

  return (
    <div className="flex flex-1 flex-col gap-3 overflow-y-auto px-4 pb-4">
      <IntegrationCard
        snapshot={snapshot}
        busy={props.busy}
        onEnable={props.onEnable}
        onDisable={props.onDisable}
      />

      <RoutingBar snapshot={snapshot} onAuto={props.onAuto} />

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
            mode={snapshot.mode}
            onPin={props.onPin}
            onUnpin={props.onAuto}
            onRename={props.onRename}
            onRemove={props.onRemove}
          />
        ))}
      </div>
    </div>
  );
}
