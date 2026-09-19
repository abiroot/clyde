import type { ReactNode } from "react";
import { useEffect, useState } from "react";
import { Check, FolderOpen, Puzzle, RefreshCw, X } from "lucide-react";
import { api } from "../lib/api";
import type { OpenChromeStatus } from "../lib/types";

interface Props {
  onClose?: () => void;
  /** Render inline (a page in the main window) instead of as a modal. */
  embedded?: boolean;
  /** Fired whenever a fresh status arrives, so the app can react to `ready`. */
  onStatus: (s: OpenChromeStatus) => void;
}

/**
 * Sets up browser tools that don't care which account is active.
 *
 * Claude Code's own Chrome pairing is per account, so every switch strands it.
 * Open Claude in Chrome talks to the browser locally instead; Clyde downloads
 * it, registers it with Claude Code once, and it then works for every account.
 * The one step Clyde can't take is loading the extension — Chrome only allows
 * that from chrome://extensions — so the checklist walks the user through it
 * and ticks itself off by watching Chrome's own files.
 */
export function BrowserToolsDialog({ onClose, onStatus, embedded = false }: Props) {
  const [status, setStatus] = useState<OpenChromeStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const accept = (s: OpenChromeStatus) => {
    setStatus(s);
    onStatus(s);
  };

  // Poll while open: loading the extension and restarting Chrome happen outside
  // Clyde, and the checklist should tick as they do.
  useEffect(() => {
    let live = true;
    const tick = () =>
      api
        .getOpenChrome()
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

  const act = async (p: Promise<OpenChromeStatus | void>) => {
    setBusy(true);
    setError(null);
    try {
      const s = await p;
      if (s) accept(s);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const wired = !!status && status.downloaded && status.deps_installed && status.mcp_registered;
  const loaded = !!status && status.loaded_in.length > 0;
  // Loaded under an id the host manifest doesn't admit yet: one more Set up fixes it.
  const needsFinish = wired && loaded && !status!.host_registered;

  const body = (
        <div className={`flex flex-col gap-3 ${embedded ? "" : "p-4"}`}>
          <p className="text-xs leading-relaxed text-[var(--color-ink-soft)]">
            Claude Code's own Chrome connection belongs to one account, so it
            drops every time you switch. This sets up{" "}
            <strong className="font-medium text-[var(--color-ink)]">
              Open Claude in Chrome
            </strong>{" "}
            instead: it talks to Chrome directly on this Mac, so it keeps
            working whichever account is active.
          </p>

          {!status ? (
            <div className="flex items-center justify-center gap-2 py-6 text-xs text-[var(--color-ink-faint)]">
              <RefreshCw size={14} className="animate-spin" /> Checking…
            </div>
          ) : (
            <ol className="flex flex-col gap-2">
              <Step n={1} done={wired} title="Download and connect it to Claude Code">
                {!status.node_path ? (
                  <Hint>
                    Needs Node.js. Install it with <code>brew install node</code>,
                    then come back.
                  </Hint>
                ) : (
                  !wired && (
                    <PrimaryButton onClick={() => act(api.setupOpenChrome())} busy={busy}>
                      {busy ? "Setting up…" : "Set up"}
                    </PrimaryButton>
                  )
                )}
              </Step>

              <Step n={2} done={loaded} title="Load it in Chrome">
                {wired && !loaded && (
                  <>
                    <Hint>
                      In the page that opens, turn on <strong>Developer mode</strong>{" "}
                      (top right), click <strong>Load unpacked</strong>, and pick the
                      folder Clyde shows you. Use one Chrome profile only. Unlike the
                      official extension it has no site blocklist, so pick a profile
                      that isn't signed into banking, crypto or payment sites.
                    </Hint>
                    <div className="flex flex-wrap gap-2">
                      <SecondaryButton onClick={() => act(api.openChromeExtensionsPage())}>
                        <Puzzle size={13} /> Open Chrome extensions
                      </SecondaryButton>
                      <SecondaryButton onClick={() => act(api.revealOpenChromeExtension())}>
                        <FolderOpen size={13} /> Show the folder
                      </SecondaryButton>
                    </div>
                  </>
                )}
                {loaded && (
                  <Hint>
                    Loaded in {status.loaded_in.join(", ")}
                    {status.loaded_in.length > 1 &&
                      " — keep it in one profile only, or the tools may land in the wrong one."}
                  </Hint>
                )}
                {needsFinish && (
                  <PrimaryButton onClick={() => act(api.setupOpenChrome())} busy={busy}>
                    {busy ? "Finishing…" : "Finish setup"}
                  </PrimaryButton>
                )}
              </Step>

              <Step n={3} done={status.connected} title="Restart Chrome">
                {loaded && !needsFinish && !status.connected && (
                  <Hint>
                    Quit Chrome completely (⌘Q) and open it again. This ticks
                    itself off once Chrome is connected.
                  </Hint>
                )}
              </Step>
            </ol>
          )}

          {status?.ready && (
            <div className="flex flex-col gap-2 rounded-xl border border-[var(--color-border-soft)] p-3">
              <p className="text-xs leading-relaxed text-[var(--color-ink-soft)]">
                Start a new <code>claude</code> session to get the tools. Running
                sessions pick them up after <code>/mcp</code> → Reconnect.
              </p>
              {status.builtin_enabled && (
                <label className="flex cursor-pointer items-start gap-2 text-xs text-[var(--color-ink-soft)]">
                  <input
                    type="checkbox"
                    className="mt-0.5 accent-[var(--color-clay)]"
                    checked={!status.builtin_enabled}
                    onChange={(e) => act(api.setBuiltinChrome(!e.target.checked))}
                  />
                  <span>
                    Turn off Claude Code's built-in Chrome tools, so sessions
                    don't see two sets (the built-in one still breaks on switch).
                  </span>
                </label>
              )}
              {!status.builtin_enabled && (
                <button
                  onClick={() => act(api.setBuiltinChrome(true))}
                  className="self-start text-xs text-[var(--color-ink-faint)] hover:text-[var(--color-ink-soft)]"
                >
                  Built-in Chrome tools are off · turn back on
                </button>
              )}
            </div>
          )}

          {error && (
            <p className="rounded-lg bg-[var(--color-danger)]/10 px-3 py-2 text-xs text-[var(--color-danger)]">
              {error}
            </p>
          )}

          {status?.downloaded && !status.reviewed && (
            <p className="rounded-lg bg-[var(--color-warn)]/10 px-3 py-2 text-xs leading-relaxed text-[var(--color-warn)]">
              The downloaded copy is at {status.commit?.slice(0, 7) ?? "an unknown version"},
              not the version Clyde reviewed. It still runs, but its code hasn't been checked.
            </p>
          )}

          {status?.downloaded && (
            <div className="flex items-center justify-between gap-2 text-[11px] text-[var(--color-ink-faint)]">
              <span className="truncate">
                {status.reviewed ? "Reviewed version" : "Version"}{" "}
                {status.commit?.slice(0, 7) ?? "?"} · noncommercial licence
              </span>
              {(status.mcp_registered || status.host_registered) && (
                <button
                  onClick={() => act(api.removeOpenChrome())}
                  className="shrink-0 hover:text-[var(--color-ink-soft)]"
                >
                  Remove
                </button>
              )}
            </div>
          )}
        </div>
  );

  if (embedded) return body;

  return (
    <div className="fixed inset-0 z-50 flex items-end justify-center bg-black/60 backdrop-blur-sm sm:items-center">
      <div className="card no-drag fade-in m-3 max-h-[92vh] w-full max-w-[420px] overflow-y-auto">
        <div className="flex items-center justify-between border-b border-[var(--color-border-soft)] px-4 py-3">
          <span className="text-sm font-semibold">Browser tools for every account</span>
          <button
            onClick={onClose}
            className="rounded-lg p-1 text-[var(--color-ink-faint)] hover:bg-white/5"
          >
            <X size={16} />
          </button>
        </div>
        {body}
      </div>
    </div>
  );
}

function Step({
  n,
  done,
  title,
  children,
}: {
  n: number;
  done: boolean;
  title: string;
  children?: ReactNode;
}) {
  return (
    <li className="flex gap-3 rounded-xl border border-[var(--color-border-soft)] p-3">
      <span
        className={`flex h-5 w-5 shrink-0 items-center justify-center rounded-full text-[11px] font-semibold ${
          done
            ? "bg-[var(--color-ok)]/15 text-[var(--color-ok)]"
            : "bg-[var(--color-surface-2)] text-[var(--color-ink-faint)]"
        }`}
      >
        {done ? <Check size={12} /> : n}
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-2">
        <span className={`text-sm ${done ? "text-[var(--color-ink-soft)]" : "font-medium"}`}>
          {title}
        </span>
        {children}
      </div>
    </li>
  );
}

function Hint({ children }: { children: ReactNode }) {
  return <p className="text-xs leading-relaxed text-[var(--color-ink-soft)]">{children}</p>;
}

function PrimaryButton({
  onClick,
  busy,
  children,
}: {
  onClick: () => void;
  busy: boolean;
  children: ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      disabled={busy}
      className="flex items-center justify-center gap-2 self-start rounded-xl bg-[var(--color-clay)] px-3 py-2 text-sm font-semibold text-[var(--n-on-accent,#1a0f0a)] hover:opacity-90 disabled:opacity-50"
    >
      {busy && <RefreshCw size={13} className="animate-spin" />}
      {children}
    </button>
  );
}

function SecondaryButton({ onClick, children }: { onClick: () => void; children: ReactNode }) {
  return (
    <button
      onClick={onClick}
      className="flex items-center gap-1.5 rounded-lg bg-[var(--color-surface-2)] px-3 py-1.5 text-xs font-medium hover:bg-white/5"
    >
      {children}
    </button>
  );
}
