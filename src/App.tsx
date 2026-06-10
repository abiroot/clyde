import { useState } from "react";
import { api } from "./lib/api";
import { useSnapshot } from "./lib/useSnapshot";
import type { AppSnapshot } from "./lib/types";
import { Dashboard } from "./views/Dashboard";
import { Onboarding } from "./views/Onboarding";
import { AddAccountDialog } from "./components/AddAccountDialog";

export default function App() {
  const { snapshot, setSnapshot } = useSnapshot();
  const [showAdd, setShowAdd] = useState(false);
  const [busy, setBusy] = useState(false);

  if (!snapshot) return <Splash />;

  const apply = (p: Promise<AppSnapshot>) => p.then(setSnapshot).catch(console.error);

  const wrapBusy = async (fn: () => Promise<AppSnapshot>) => {
    setBusy(true);
    try {
      setSnapshot(await fn());
    } catch (e) {
      console.error(e);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="relative z-10 flex h-full flex-col">
      <TitleBar snapshot={snapshot} />

      {snapshot.accounts.length === 0 ? (
        <Onboarding onAdd={() => setShowAdd(true)} />
      ) : (
        <Dashboard
          snapshot={snapshot}
          busy={busy}
          onAdd={() => setShowAdd(true)}
          onPin={(id) => apply(api.setMode({ kind: "pinned", accountId: id }))}
          onAuto={() => apply(api.setMode({ kind: "auto" }))}
          onRename={(id, label) => apply(api.renameAccount(id, label))}
          onRemove={(id) => apply(api.removeAccount(id))}
          onEnable={() => wrapBusy(api.enableIntegration)}
          onDisable={() => wrapBusy(api.disableIntegration)}
        />
      )}

      {showAdd && (
        <AddAccountDialog
          onClose={() => setShowAdd(false)}
          onDone={(s) => {
            setSnapshot(s);
            setShowAdd(false);
          }}
        />
      )}
    </div>
  );
}

function TitleBar({ snapshot }: { snapshot: AppSnapshot }) {
  const ok = snapshot.proxy_running;
  return (
    <div className="drag flex items-center justify-between px-4 pb-3 pt-3.5 pl-[78px]">
      <div className="flex items-center gap-2">
        <span className="text-sm font-semibold tracking-tight">Clyde</span>
        <span className="text-[11px] text-[var(--color-ink-faint)]">
          Claude account switcher
        </span>
      </div>
      <div
        className="flex items-center gap-1.5 text-[11px]"
        title={ok ? "Proxy running" : "Proxy offline"}
      >
        <span
          className="h-2 w-2 rounded-full"
          style={{
            background: ok ? "var(--color-ok)" : "var(--color-ink-faint)",
            boxShadow: ok ? "0 0 6px var(--color-ok)" : "none",
          }}
        />
        <span className="text-[var(--color-ink-faint)]">{ok ? "live" : "offline"}</span>
      </div>
    </div>
  );
}

function Splash() {
  return (
    <div className="flex h-full items-center justify-center">
      <div
        className="h-10 w-10 animate-pulse rounded-2xl"
        style={{ background: "var(--color-clay)" }}
      />
    </div>
  );
}
