import type { ReactNode } from "react";
import { useEffect, useState } from "react";
import { Check, RefreshCw } from "lucide-react";
import { api } from "../lib/api";
import type { BrowserStatus, BrowserTest, ChromeProfile } from "../lib/types";
import { Group, GroupRow, Toggle } from "../ui/kit";

interface Props {
  /** Fired whenever a fresh status arrives, so the app can react to `ready`. */
  onStatus: (s: BrowserStatus) => void;
}

/**
 * Browser tools that don't care which account is active.
 *
 * Claude Code's own Chrome pairing is per account, so every switch strands it.
 * Playwright MCP talks to Chrome through Microsoft's Playwright Extension
 * instead, over a local connection with a per-profile token. The only step
 * Clyde can't take is the Web Store install; it finds the token itself and
 * registers the server with Claude Code. The checklist ticks itself off by
 * watching Chrome's own files.
 */
export function BrowserPage({ onStatus }: Props) {
  const [status, setStatus] = useState<BrowserStatus | null>(null);
  const [profile, setProfile] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [needToken, setNeedToken] = useState(false);
  const [token, setToken] = useState("");
  const [test, setTest] = useState<BrowserTest | null>(null);

  const accept = (s: BrowserStatus) => {
    setStatus(s);
    onStatus(s);
  };

  // Poll while open: the Web Store install happens in Chrome, and the
  // checklist should tick as it does.
  useEffect(() => {
    let live = true;
    const tick = () =>
      api
        .getBrowser()
        .then((s) => live && accept(s))
        .catch(() => {});
    tick();
    const t = setInterval(tick, 3000);
    return () => {
      live = false;
      clearInterval(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Default to the registered profile, else one that has the extension.
  useEffect(() => {
    if (!status || profile) return;
    setProfile(
      status.profile ??
        status.profiles.find((p) => p.has_extension)?.dir ??
        status.profiles[0]?.dir ??
        "Default",
    );
  }, [status, profile]);

  const act = async (name: string, p: Promise<BrowserStatus | void>) => {
    setBusy(name);
    setError(null);
    try {
      const s = await p;
      if (s) accept(s);
      return true;
    } catch (e) {
      const msg = String(e);
      setError(msg);
      if (/token/i.test(msg)) setNeedToken(true);
      return false;
    } finally {
      setBusy(null);
    }
  };

  const connect = async () => {
    if (!profile) return;
    setTest(null);
    const ok = await act("connect", api.browserSetup(profile, needToken && token.trim() ? token.trim() : null));
    if (ok) {
      setNeedToken(false);
      setToken("");
    }
  };

  const runTest = async () => {
    setBusy("test");
    setTest(null);
    try {
      setTest(await api.browserTest());
    } catch (e) {
      setTest({ ok: false, detail: String(e) });
    } finally {
      setBusy(null);
    }
  };

  if (!status) {
    return (
      <div className="flex max-w-[600px] flex-col gap-6">
        <div className="n-group h-[148px] animate-pulse" />
        <div className="n-group h-[52px] animate-pulse" />
      </div>
    );
  }

  const selected = status.profiles.find((p) => p.dir === profile) ?? null;
  const installed = !!selected?.has_extension;
  const connectedHere = status.ready && status.profile === profile;
  const changingProfile = status.ready && status.profile !== profile;

  return (
    <div className="flex max-w-[600px] flex-col gap-6">
      <Group
        title={status.ready ? undefined : "Set up"}
        footer={
          <>
            Claude Code's own Chrome connection belongs to one account, so it drops whenever you switch. These tools
            use Microsoft's Playwright Extension instead, which talks to Chrome on this Mac, so they work with every
            account. Each session gets its own tab group and only sees the tabs in it.
          </>
        }
      >
        {!status.node_found && (
          <GroupRow>
            <span className="flex-1" style={{ color: "var(--n-warn)" }}>
              Needs Node.js. Install it with <code>brew install node</code>, then come back.
            </span>
          </GroupRow>
        )}

        <GroupRow>
          <span className="flex-1">
            Chrome profile
            <span className="n-dim block text-[11px]">
              Pick one without banking or crypto logins; the agent can use whatever this profile is signed into.
            </span>
          </span>
          <ProfileSelect profiles={status.profiles} value={profile} onChange={(p) => { setProfile(p); setTest(null); }} />
        </GroupRow>

        <Step done={installed} n={1} title="Install the Playwright Extension">
          {!installed && (
            <button className="n-plain-button" onClick={() => profile && act("store", api.browserOpenStore(profile))}>
              Open Chrome Web Store
            </button>
          )}
          {installed && <span className="n-dim text-[12px]">Installed</span>}
        </Step>

        <Step done={connectedHere} n={2} title="Connect it to Claude Code">
          {(!connectedHere || status.token_stale) && (
            <button
              className="n-button"
              disabled={!installed || !status.node_found || busy !== null}
              onClick={connect}
              style={{ padding: "2px 12px" }}
            >
              {busy === "connect" ? "Connecting…" : changingProfile ? "Use this profile" : status.token_stale ? "Reconnect" : "Connect"}
            </button>
          )}
          {connectedHere && !status.token_stale && <span className="n-dim text-[12px]">Connected</span>}
        </Step>

        {status.token_stale && (
          <GroupRow>
            <span className="flex-1 text-[12px]" style={{ color: "var(--n-warn)" }}>
              The extension was reinstalled, so its token changed. Reconnect to keep the tools working.
            </span>
          </GroupRow>
        )}

        {needToken && (
          <GroupRow className="flex-col !items-stretch gap-2">
            <span className="text-[12px] leading-relaxed">
              Click the Playwright icon in Chrome's toolbar (in the puzzle-piece menu) and Clyde will pick the token up.
              Or copy the token shown there and paste it here:
            </span>
            <div className="flex gap-2">
              <input
                value={token}
                onChange={(e) => setToken(e.target.value)}
                placeholder="PLAYWRIGHT_MCP_EXTENSION_TOKEN=…"
                spellCheck={false}
                className="min-w-0 flex-1 rounded-md border border-[var(--n-card-border)] bg-[var(--n-window)] px-2 py-1 font-mono text-[12px]"
              />
              <button className="n-plain-button" disabled={busy !== null} onClick={connect}>
                Connect
              </button>
            </div>
          </GroupRow>
        )}

        {status.ready && (
          <GroupRow>
            <span className="flex-1">
              {test ? (
                <span style={{ color: test.ok ? "var(--n-ok)" : "var(--n-danger)" }}>{test.detail}</span>
              ) : (
                <>
                  Ready for every account
                  <span className="n-dim block text-[11px]">
                    New <code>claude</code> sessions get the <code>playwright</code> tools; running ones after{" "}
                    <code>/mcp</code> → Reconnect.
                  </span>
                </>
              )}
            </span>
            <button className="n-plain-button flex items-center gap-1.5" disabled={busy !== null} onClick={runTest}>
              {busy === "test" && <RefreshCw size={11} className="animate-spin" />}
              {busy === "test" ? "Testing…" : "Test connection"}
            </button>
          </GroupRow>
        )}
      </Group>

      {error && (
        <p className="-mt-3 px-1 text-[12px]" style={{ color: "var(--n-danger)" }}>
          {error}
        </p>
      )}

      <Group
        title="Claude Code's own Chrome tools"
        footer="These only work while Chrome's Claude extension is signed into the same account as Claude Code. Turn them off so sessions don't see two sets of browser tools."
      >
        <GroupRow>
          <span className="flex-1">Built-in Claude in Chrome</span>
          <Toggle
            label="Built-in Claude in Chrome"
            checked={status.builtin_enabled}
            onChange={(v) => act("builtin", api.setBuiltinChrome(v))}
          />
        </GroupRow>
      </Group>

      {status.legacy_present && (
        <Group
          title="Open Claude in Chrome"
          footer="Clyde 1.0 set this up. It's replaced by the tools above. After removing it here, also remove its extension from chrome://extensions."
        >
          <GroupRow>
            <span className="flex-1">Still connected to Claude Code</span>
            <button
              className="n-plain-button"
              disabled={busy !== null}
              onClick={() => act("legacy", api.browserRemoveLegacy())}
            >
              Remove
            </button>
            <button className="n-plain-button" onClick={() => profile && act("ext", api.openChromeExtensionsPage(profile))}>
              Open extensions
            </button>
          </GroupRow>
        </Group>
      )}

      {status.mcp_registered && (
        <div className="n-dim flex items-center justify-between px-1 text-[11px]">
          <span>
            Playwright MCP {status.registered_version ?? "?"}
            {status.registered_version && status.registered_version !== status.pinned_version
              ? ` · Clyde would use ${status.pinned_version}; Reconnect to switch`
              : ""}
          </span>
          <button className="hover:text-[var(--n-text)]" disabled={busy !== null} onClick={() => act("remove", api.browserRemove())}>
            Disconnect from Claude Code
          </button>
        </div>
      )}
    </div>
  );
}

function Step({ n, done, title, children }: { n: number; done: boolean; title: string; children?: ReactNode }) {
  return (
    <GroupRow>
      <span
        className="grid h-[18px] w-[18px] shrink-0 place-items-center rounded-full text-[10px] font-semibold"
        style={{
          background: done ? "color-mix(in srgb, var(--n-ok) 18%, transparent)" : "var(--n-track)",
          color: done ? "var(--n-ok)" : "var(--n-text-2)",
        }}
      >
        {done ? <Check size={11} strokeWidth={2.5} /> : n}
      </span>
      <span className={`flex-1 ${done ? "n-dim" : ""}`}>{title}</span>
      {children}
    </GroupRow>
  );
}

function ProfileSelect({
  profiles,
  value,
  onChange,
}: {
  profiles: ChromeProfile[];
  value: string | null;
  onChange: (dir: string) => void;
}) {
  if (profiles.length === 0) return <span className="n-dim text-[12px]">Chrome not found</span>;
  return (
    <select
      value={value ?? ""}
      onChange={(e) => onChange(e.target.value)}
      className="max-w-[260px] rounded-md border border-[var(--n-card-border)] bg-[var(--n-window)] px-2 py-0.5 text-[13px]"
    >
      {profiles.map((p) => (
        <option key={p.dir} value={p.dir}>
          {p.name}
          {p.email ? ` — ${p.email}` : ""}
        </option>
      ))}
    </select>
  );
}
