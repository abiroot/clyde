import { useEffect, useState } from "react";
import { Activity, Gauge, Globe, Plus, Settings2, TerminalSquare } from "lucide-react";
import { api } from "./lib/api";
import { useSnapshot } from "./lib/useSnapshot";
import type { OpenChromeStatus } from "./lib/types";
import { AddAccountDialog } from "./components/AddAccountDialog";
import { BrowserToolsDialog } from "./components/BrowserToolsDialog";
import { OverviewPage } from "./pages/OverviewPage";
import { UsagePage } from "./pages/UsagePage";
import { SessionsPage } from "./pages/SessionsPage";
import { SettingsPage } from "./pages/SettingsPage";
import { Onboarding } from "./views/Onboarding";

type Page = "overview" | "usage" | "sessions" | "browser" | "settings";

const NAV: { id: Page; label: string; icon: typeof Gauge }[] = [
  { id: "overview", label: "Accounts", icon: Gauge },
  { id: "usage", label: "Usage", icon: Activity },
  { id: "sessions", label: "Sessions", icon: TerminalSquare },
  { id: "browser", label: "Browser tools", icon: Globe },
  { id: "settings", label: "Settings", icon: Settings2 },
];

function pageFromHash(): Page {
  const h = window.location.hash.slice(1) as Page;
  return NAV.some((n) => n.id === h) ? h : "overview";
}

/**
 * The full window: a translucent sidebar (frosted by macOS) and an opaque
 * content pane, laid out like System Settings. Quick glances live in the
 * menubar popover; this is for managing accounts, history, sessions and setup.
 */
export default function App() {
  const { snapshot, setSnapshot } = useSnapshot();
  const [page, setPageState] = useState<Page>(pageFromHash);
  // The page lives in the URL hash, so it survives reloads and can be linked.
  const setPage = (p: Page) => {
    window.location.hash = p;
    setPageState(p);
  };
  useEffect(() => {
    const on = () => setPageState(pageFromHash());
    window.addEventListener("hashchange", on);
    return () => window.removeEventListener("hashchange", on);
  }, []);
  const [showAdd, setShowAdd] = useState(false);
  const [openChrome, setOpenChrome] = useState<OpenChromeStatus | null>(null);

  useEffect(() => {
    api.getOpenChrome().then(setOpenChrome).catch(() => setOpenChrome(null));
  }, [snapshot?.active_id]);

  const current = NAV.find((n) => n.id === page)!;

  return (
    <div className="flex h-full">
      <aside data-tauri-drag-region className="flex w-[196px] shrink-0 flex-col gap-0.5 px-2.5 pb-3 pt-[46px]">
        {NAV.map(({ id, label, icon: Icon }) => (
          <button
            key={id}
            className="n-sidebar-item"
            aria-current={page === id ? "page" : undefined}
            onClick={() => setPage(id)}
          >
            <Icon size={15} strokeWidth={1.75} className="text-[var(--n-accent)]" />
            {label}
          </button>
        ))}
        <div className="flex-1" data-tauri-drag-region />
        <button className="n-sidebar-item n-dim" onClick={() => setShowAdd(true)}>
          <Plus size={15} strokeWidth={1.75} />
          Add account
        </button>
      </aside>

      <main className="flex min-w-0 flex-1 flex-col bg-[var(--n-window)]">
        <header data-tauri-drag-region className="flex h-[46px] shrink-0 items-center px-6">
          <h1 data-tauri-drag-region className="text-[15px] font-semibold">
            {current.label}
          </h1>
        </header>

        <div className="flex-1 overflow-y-auto px-6 pb-8">
          {!snapshot ? null : snapshot.accounts.length === 0 && page === "overview" ? (
            <Onboarding onAdd={() => setShowAdd(true)} />
          ) : page === "overview" ? (
            <OverviewPage
              snapshot={snapshot}
              setSnapshot={setSnapshot}
              browserReady={!!openChrome?.ready}
              onOpenBrowserTools={() => setPage("browser")}
              onAdd={() => setShowAdd(true)}
            />
          ) : page === "usage" ? (
            <UsagePage snapshot={snapshot} />
          ) : page === "sessions" ? (
            <SessionsPage />
          ) : page === "browser" ? (
            <div className="max-w-[560px]">
              <BrowserToolsDialog embedded onStatus={setOpenChrome} />
            </div>
          ) : (
            <SettingsPage />
          )}
        </div>
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
