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
  const [busyId, setBusyId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  if (!snapshot) return <Splash />;

  const apply = (p: Promise<AppSnapshot>) => p.then(setSnapshot).catch(console.error);

  const activate = async (id: string) => {
    setBusyId(id);
    setError(null);
    try {
      setSnapshot(await api.setActiveAccount(id));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusyId(null);
    }
  };

  return (
    <div className="relative z-10 flex h-full flex-col">
      <TitleBar snapshot={snapshot} />

      <main className="flex flex-1 flex-col overflow-hidden">
        {error && (
          <div className="mx-4 mt-1 mb-2 flex items-start justify-between gap-2 rounded-lg border border-[var(--color-danger)]/30 bg-[var(--color-danger)]/10 px-3 py-2 text-[11px] text-[var(--color-danger)]">
            <span className="leading-snug">Couldn't switch account: {error}</span>
            <button
              onClick={() => setError(null)}
              className="shrink-0 font-medium hover:opacity-70"
            >
              Dismiss
            </button>
          </div>
        )}

        {snapshot.accounts.length === 0 ? (
          <Onboarding onAdd={() => setShowAdd(true)} />
        ) : (
          <Dashboard
            snapshot={snapshot}
            busyId={busyId}
            onAdd={() => setShowAdd(true)}
            onActivate={activate}
            onRename={(id, label) => apply(api.renameAccount(id, label))}
            onRemove={(id) => apply(api.removeAccount(id))}
          />
        )}
      </main>

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

/**
 * The window's draggable title bar. Uses `data-tauri-drag-region` (the OS-level
 * drag API) rather than the `-webkit-app-region` CSS hack, which is unreliable
 * on macOS. Decorative children are `pointer-events-none` so a mousedown lands
 * on the header itself and starts the drag; the left inset clears the native
 * traffic-light buttons.
 */
function TitleBar({ snapshot }: { snapshot: AppSnapshot }) {
  const active =
    snapshot.active_email ??
    snapshot.accounts.find((a) => a.id === snapshot.active_id)?.label;

  return (
    <header
      data-tauri-drag-region
      className="relative flex h-12 shrink-0 select-none items-center justify-between gap-3 pl-[78px] pr-4"
    >
      <div
        data-tauri-drag-region
        className="pointer-events-none flex min-w-0 items-center gap-2"
      >
        <span className="text-sm font-semibold tracking-tight">Clyde</span>
        <span className="truncate text-[11px] text-[var(--color-ink-faint)]">
          Claude account switcher
        </span>
      </div>

      {active && (
        <div
          data-tauri-drag-region
          className="pointer-events-none flex shrink-0 items-center gap-1.5 text-[11px] text-[var(--color-ink-faint)]"
        >
          <span
            className="h-2 w-2 rounded-full"
            style={{
              background: "var(--color-ok)",
              boxShadow: "0 0 6px var(--color-ok)",
            }}
          />
          <span className="max-w-[160px] truncate">{active}</span>
        </div>
      )}
    </header>
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
