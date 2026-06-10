import type { ReactNode } from "react";
import { useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Globe, ClipboardPaste, X, ExternalLink } from "lucide-react";
import { api } from "../lib/api";
import type { AppSnapshot } from "../lib/types";

interface Props {
  onDone: (snapshot: AppSnapshot) => void;
  onClose: () => void;
}

type Tab = "browser" | "token";

export function AddAccountDialog({ onDone, onClose }: Props) {
  const [tab, setTab] = useState<Tab>("browser");
  const [label, setLabel] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Browser flow
  const [flowId, setFlowId] = useState<string | null>(null);
  const [code, setCode] = useState("");

  // Token flow
  const [tokenJson, setTokenJson] = useState("");

  async function startBrowserLogin() {
    setError(null);
    setBusy(true);
    try {
      const { flow_id, authorize_url } = await api.beginLogin();
      setFlowId(flow_id);
      await openUrl(authorize_url);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function finishBrowserLogin() {
    if (!flowId) return;
    setError(null);
    setBusy(true);
    try {
      const snap = await api.completeLogin(flowId, code.trim(), label.trim());
      onDone(snap);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function importToken() {
    setError(null);
    setBusy(true);
    try {
      const snap = await api.importToken(label.trim(), tokenJson.trim());
      onDone(snap);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-end justify-center bg-black/60 backdrop-blur-sm sm:items-center">
      <div className="card no-drag fade-in m-3 w-full max-w-[420px] overflow-hidden">
        {/* Header */}
        <div className="flex items-center justify-between border-b border-[var(--color-border-soft)] px-4 py-3">
          <span className="text-sm font-semibold">Add a Claude account</span>
          <button
            onClick={onClose}
            className="rounded-lg p-1 text-[var(--color-ink-faint)] hover:bg-white/5"
          >
            <X size={16} />
          </button>
        </div>

        {/* Tabs */}
        <div className="flex gap-1 px-4 pt-3">
          <TabButton active={tab === "browser"} onClick={() => setTab("browser")}>
            <Globe size={14} /> Browser sign-in
          </TabButton>
          <TabButton active={tab === "token"} onClick={() => setTab("token")}>
            <ClipboardPaste size={14} /> Paste token
          </TabButton>
        </div>

        <div className="flex flex-col gap-3 p-4">
          <Field label="Account name">
            <input
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              placeholder="e.g. Personal Max, Work Max"
              className="w-full rounded-lg bg-[var(--color-surface-2)] px-3 py-2 text-sm outline-none focus:ring-1 focus:ring-[var(--color-clay)]/50"
            />
          </Field>

          {tab === "browser" ? (
            <>
              {!flowId ? (
                <button
                  disabled={busy}
                  onClick={startBrowserLogin}
                  className="flex items-center justify-center gap-2 rounded-xl bg-[var(--color-clay)] px-3 py-2.5 text-sm font-semibold text-[#1a0f0a] hover:opacity-90 disabled:opacity-50"
                >
                  <ExternalLink size={15} />
                  {busy ? "Opening…" : "Open Claude sign-in"}
                </button>
              ) : (
                <>
                  <p className="text-xs leading-relaxed text-[var(--color-ink-soft)]">
                    After authorizing in your browser, copy the code you're shown and paste it
                    below.
                  </p>
                  <Field label="Authorization code">
                    <input
                      value={code}
                      onChange={(e) => setCode(e.target.value)}
                      placeholder="paste code here"
                      className="w-full rounded-lg bg-[var(--color-surface-2)] px-3 py-2 font-mono text-xs outline-none focus:ring-1 focus:ring-[var(--color-clay)]/50"
                    />
                  </Field>
                  <button
                    disabled={busy || !code.trim()}
                    onClick={finishBrowserLogin}
                    className="rounded-xl bg-[var(--color-clay)] px-3 py-2.5 text-sm font-semibold text-[#1a0f0a] hover:opacity-90 disabled:opacity-40"
                  >
                    {busy ? "Verifying…" : "Add account"}
                  </button>
                </>
              )}
            </>
          ) : (
            <>
              <Field label="Token JSON">
                <textarea
                  value={tokenJson}
                  onChange={(e) => setTokenJson(e.target.value)}
                  rows={5}
                  placeholder='{ "accessToken": "…", "refreshToken": "…", "expiresAt": 0 }'
                  className="w-full resize-none rounded-lg bg-[var(--color-surface-2)] px-3 py-2 font-mono text-xs outline-none focus:ring-1 focus:ring-[var(--color-clay)]/50"
                />
              </Field>
              <button
                disabled={busy || !tokenJson.trim()}
                onClick={importToken}
                className="rounded-xl bg-[var(--color-clay)] px-3 py-2.5 text-sm font-semibold text-[#1a0f0a] hover:opacity-90 disabled:opacity-40"
              >
                {busy ? "Importing…" : "Import account"}
              </button>
            </>
          )}

          {error && (
            <p className="rounded-lg bg-[var(--color-danger)]/10 px-3 py-2 text-xs text-[var(--color-danger)]">
              {error}
            </p>
          )}
        </div>
      </div>
    </div>
  );
}

function TabButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      className={`flex flex-1 items-center justify-center gap-1.5 rounded-lg px-2 py-1.5 text-xs font-medium transition-colors ${
        active
          ? "bg-[var(--color-surface-2)] text-[var(--color-ink)]"
          : "text-[var(--color-ink-faint)] hover:text-[var(--color-ink-soft)]"
      }`}
    >
      {children}
    </button>
  );
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label className="flex flex-col gap-1.5">
      <span className="text-[11px] uppercase tracking-wide text-[var(--color-ink-faint)]">
        {label}
      </span>
      {children}
    </label>
  );
}
