import type { ReactNode } from "react";
import { useEffect, useState } from "react";
import {
  Download,
  UserPlus,
  ClipboardPaste,
  X,
  Terminal,
  Check,
  RefreshCw,
  Globe,
} from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api } from "../lib/api";
import type { AppSnapshot, Discovered } from "../lib/types";

interface Props {
  onDone: (snapshot: AppSnapshot) => void;
  onClose: () => void;
}

type Tab = "claude" | "browser" | "new" | "token";

export function AddAccountDialog({ onDone, onClose }: Props) {
  const [tab, setTab] = useState<Tab>("claude");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  return (
    <div className="fixed inset-0 z-50 flex items-end justify-center bg-black/60 backdrop-blur-sm sm:items-center">
      <div className="card no-drag fade-in m-3 w-full max-w-[420px] overflow-hidden">
        <div className="flex items-center justify-between border-b border-[var(--color-border-soft)] px-4 py-3">
          <span className="text-sm font-semibold">Add a Claude account</span>
          <button
            onClick={onClose}
            className="rounded-lg p-1 text-[var(--color-ink-faint)] hover:bg-white/5"
          >
            <X size={16} />
          </button>
        </div>

        <div className="flex gap-1 px-4 pt-3">
          <TabButton active={tab === "claude"} onClick={() => setTab("claude")}>
            <Download size={14} /> Detected
          </TabButton>
          <TabButton active={tab === "browser"} onClick={() => setTab("browser")}>
            <Globe size={14} /> Browser
          </TabButton>
          <TabButton active={tab === "new"} onClick={() => setTab("new")}>
            <UserPlus size={14} /> Terminal
          </TabButton>
          <TabButton active={tab === "token"} onClick={() => setTab("token")}>
            <ClipboardPaste size={14} /> Token
          </TabButton>
        </div>

        <div className="flex flex-col gap-3 p-4">
          {tab === "claude" && (
            <ImportTab busy={busy} setBusy={setBusy} setError={setError} onDone={onDone} />
          )}
          {tab === "browser" && (
            <BrowserTab busy={busy} setBusy={setBusy} setError={setError} onDone={onDone} />
          )}
          {tab === "new" && (
            <NewAccountTab busy={busy} setBusy={setBusy} setError={setError} onDone={onDone} />
          )}
          {tab === "token" && (
            <TokenTab busy={busy} setBusy={setBusy} setError={setError} onDone={onDone} />
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

interface TabProps {
  busy: boolean;
  setBusy: (b: boolean) => void;
  setError: (e: string | null) => void;
  onDone: (s: AppSnapshot) => void;
}

/** The recommended path: reuse the accounts Claude Code already logged in. */
function ImportTab({ busy, setBusy, setError, onDone }: TabProps) {
  const [found, setFound] = useState<Discovered[] | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());

  const scan = async () => {
    setError(null);
    setBusy(true);
    try {
      const list = await api.discoverClaudeAccounts();
      setFound(list);
      setSelected(new Set(list.map((d) => d.config_dir)));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  useEffect(() => {
    scan();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const toggle = (dir: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      next.has(dir) ? next.delete(dir) : next.add(dir);
      return next;
    });
  };

  const doImport = async () => {
    setError(null);
    setBusy(true);
    try {
      const snap = await api.importClaudeAccounts([...selected]);
      onDone(snap);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  if (found === null) {
    return (
      <div className="flex items-center justify-center gap-2 py-6 text-xs text-[var(--color-ink-faint)]">
        <RefreshCw size={14} className="animate-spin" /> Scanning for Claude accounts…
      </div>
    );
  }

  if (found.length === 0) {
    return (
      <div className="flex flex-col gap-2 py-2">
        <p className="text-xs leading-relaxed text-[var(--color-ink-soft)]">
          No signed-in Claude accounts found on this machine. Use the{" "}
          <strong className="font-medium text-[var(--color-ink)]">Browser</strong> tab to
          sign in to one now, or the Terminal / Token tabs.
        </p>
        <button
          onClick={scan}
          className="self-start rounded-lg bg-[var(--color-surface-2)] px-3 py-1.5 text-xs font-medium hover:bg-white/5"
        >
          Scan again
        </button>
      </div>
    );
  }

  return (
    <>
      <p className="text-xs leading-relaxed text-[var(--color-ink-soft)]">
        Found {found.length} signed-in {found.length === 1 ? "account" : "accounts"}. Clyde
        reuses their tokens — no re-login needed.
      </p>
      <div className="flex flex-col gap-1.5">
        {found.map((d) => {
          const on = selected.has(d.config_dir);
          return (
            <button
              key={d.config_dir}
              onClick={() => toggle(d.config_dir)}
              className={`flex items-center gap-3 rounded-xl border p-2.5 text-left transition-colors ${
                on
                  ? "border-[var(--color-clay)]/40 bg-[var(--color-clay)]/5"
                  : "border-[var(--color-border-soft)] hover:bg-white/5"
              }`}
            >
              <span
                className={`grid h-5 w-5 shrink-0 place-items-center rounded-md border ${
                  on
                    ? "border-[var(--color-clay)] bg-[var(--color-clay)] text-[#1a0f0a]"
                    : "border-[var(--color-border)]"
                }`}
              >
                {on && <Check size={13} />}
              </span>
              <span className="min-w-0 flex-1">
                <span className="block truncate text-sm font-medium">{d.label}</span>
                <span className="block truncate text-[11px] text-[var(--color-ink-faint)]">
                  {d.subscription_type ?? d.config_dir}
                </span>
              </span>
            </button>
          );
        })}
      </div>
      <button
        disabled={busy || selected.size === 0}
        onClick={doImport}
        className="rounded-xl bg-[var(--color-clay)] px-3 py-2.5 text-sm font-semibold text-[#1a0f0a] hover:opacity-90 disabled:opacity-40"
      >
        {busy ? "Importing…" : `Import ${selected.size} ${selected.size === 1 ? "account" : "accounts"}`}
      </button>
    </>
  );
}

/** Sign a new account in through the browser (PKCE), no terminal needed. The
 *  user signs in, copies the code Anthropic shows, and pastes it back. */
function BrowserTab({ busy, setBusy, setError, onDone }: TabProps) {
  const [flow, setFlow] = useState<{ flowId: string; url: string } | null>(null);
  const [label, setLabel] = useState("");
  const [code, setCode] = useState("");

  const start = async () => {
    setError(null);
    setBusy(true);
    try {
      const { flow_id, authorize_url } = await api.beginLogin();
      setFlow({ flowId: flow_id, url: authorize_url });
      await openUrl(authorize_url);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const complete = async () => {
    if (!flow || !code.trim()) return;
    setError(null);
    setBusy(true);
    try {
      onDone(await api.completeLogin(flow.flowId, code.trim(), label.trim()));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  if (!flow) {
    return (
      <>
        <p className="text-xs leading-relaxed text-[var(--color-ink-soft)]">
          Sign in to any Claude account in your browser — no terminal needed. Clyde
          reads its email and plan automatically once you paste the code back.
        </p>
        <button
          disabled={busy}
          onClick={start}
          className="flex items-center justify-center gap-2 rounded-xl bg-[var(--color-clay)] px-3 py-2.5 text-sm font-semibold text-[#1a0f0a] hover:opacity-90 disabled:opacity-50"
        >
          <Globe size={15} />
          {busy ? "Opening browser…" : "Sign in with browser"}
        </button>
      </>
    );
  }

  return (
    <>
      <div className="rounded-xl border border-[var(--color-border-soft)] p-3">
        <ol className="flex flex-col gap-1.5 text-xs text-[var(--color-ink-soft)]">
          <li>1. A browser opened — sign in to the account you want to add.</li>
          <li>2. Anthropic shows an authorization code. Copy it.</li>
          <li>3. Paste it below and finish.</li>
        </ol>
      </div>
      <Field label="Account name (optional)">
        <input
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          placeholder="Defaults to the account's email"
          className="w-full rounded-lg bg-[var(--color-surface-2)] px-3 py-2 text-sm outline-none focus:ring-1 focus:ring-[var(--color-clay)]/50"
        />
      </Field>
      <Field label="Authorization code">
        <input
          value={code}
          onChange={(e) => setCode(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && complete()}
          autoFocus
          placeholder="Paste the code from your browser"
          className="w-full rounded-lg bg-[var(--color-surface-2)] px-3 py-2 font-mono text-xs outline-none focus:ring-1 focus:ring-[var(--color-clay)]/50"
        />
      </Field>
      <button
        disabled={busy || !code.trim()}
        onClick={complete}
        className="rounded-xl bg-[var(--color-clay)] px-3 py-2.5 text-sm font-semibold text-[#1a0f0a] hover:opacity-90 disabled:opacity-40"
      >
        {busy ? "Signing in…" : "Finish sign-in"}
      </button>
      <button
        onClick={() => openUrl(flow.url)}
        className="text-xs text-[var(--color-ink-faint)] hover:text-[var(--color-ink-soft)]"
      >
        Reopen browser
      </button>
    </>
  );
}

/** Add an account that isn't on this machine yet, by letting Claude Code's own
 *  login do the work (its OAuth is the maintained one), then importing it. */
function NewAccountTab({ busy, setBusy, setError, onDone }: TabProps) {
  const [dir, setDir] = useState<string | null>(null);

  const openLogin = async () => {
    setError(null);
    setBusy(true);
    try {
      setDir(await api.startClaudeLogin());
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const finishImport = async () => {
    if (!dir) return;
    setError(null);
    setBusy(true);
    try {
      onDone(await api.importClaudeAccounts([dir]));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  if (!dir) {
    return (
      <>
        <p className="text-xs leading-relaxed text-[var(--color-ink-soft)]">
          Sign a new account in using Claude Code's own login. Clyde opens a terminal running{" "}
          <code>claude</code> in an isolated profile — sign in there, then come back and import.
        </p>
        <button
          disabled={busy}
          onClick={openLogin}
          className="flex items-center justify-center gap-2 rounded-xl bg-[var(--color-clay)] px-3 py-2.5 text-sm font-semibold text-[#1a0f0a] hover:opacity-90 disabled:opacity-50"
        >
          <Terminal size={15} />
          {busy ? "Opening terminal…" : "Open Claude sign-in"}
        </button>
      </>
    );
  }

  return (
    <>
      <div className="rounded-xl border border-[var(--color-border-soft)] p-3">
        <ol className="flex flex-col gap-1.5 text-xs text-[var(--color-ink-soft)]">
          <li>1. A Terminal window opened running <code>claude</code>.</li>
          <li>2. Use <code>/login</code> there and finish signing in.</li>
          <li>3. Come back and click Import below.</li>
        </ol>
      </div>
      <button
        disabled={busy}
        onClick={finishImport}
        className="rounded-xl bg-[var(--color-clay)] px-3 py-2.5 text-sm font-semibold text-[#1a0f0a] hover:opacity-90 disabled:opacity-50"
      >
        {busy ? "Importing…" : "I've signed in — import"}
      </button>
      <button
        onClick={openLogin}
        className="text-xs text-[var(--color-ink-faint)] hover:text-[var(--color-ink-soft)]"
      >
        Reopen terminal
      </button>
    </>
  );
}

function TokenTab({ busy, setBusy, setError, onDone }: TabProps) {
  const [label, setLabel] = useState("");
  const [tokenJson, setTokenJson] = useState("");

  const submit = async () => {
    setError(null);
    setBusy(true);
    try {
      onDone(await api.importToken(label.trim(), tokenJson.trim()));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <Field label="Account name">
        <input
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          placeholder="e.g. Work Max"
          className="w-full rounded-lg bg-[var(--color-surface-2)] px-3 py-2 text-sm outline-none focus:ring-1 focus:ring-[var(--color-clay)]/50"
        />
      </Field>
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
        onClick={submit}
        className="rounded-xl bg-[var(--color-clay)] px-3 py-2.5 text-sm font-semibold text-[#1a0f0a] hover:opacity-90 disabled:opacity-40"
      >
        {busy ? "Importing…" : "Import account"}
      </button>
    </>
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
      className={`flex flex-1 items-center justify-center gap-1.5 whitespace-nowrap rounded-lg px-2 py-1.5 text-xs font-medium transition-colors ${
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
