import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { api } from "../lib/api";
import { useSnapshot } from "../lib/useSnapshot";
import type { AccountView, OpenChromeStatus } from "../lib/types";
import {
  AccountSummary,
  Avatar,
  LimitRow,
  ago,
  limitRows,
  loginBroken,
  shortReset,
  tightest,
  toneColor,
} from "../ui/kit";

const WIDTH = 340;

/**
 * The menubar popover: the active account's limits at a glance, every other
 * account's headroom with a one-click switch, and a way into the full window.
 * Styled like a native macOS menubar extra — frosted, system font, follows
 * light/dark.
 */
export function Popover() {
  const { snapshot, setSnapshot } = useSnapshot();
  const [browser, setBrowser] = useState<OpenChromeStatus | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const root = useRef<HTMLDivElement>(null);

  // Refresh whenever the popover opens: it may have been hidden for hours.
  useEffect(() => {
    const refresh = () => {
      api.getSnapshot().then(setSnapshot).catch(() => {});
      api.getOpenChrome().then(setBrowser).catch(() => setBrowser(null));
    };
    refresh();
    const un = getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (focused) refresh();
      else setNotice(null);
    });
    return () => {
      un.then((f) => f());
    };
  }, [setSnapshot]);

  // Size the window to its content, like a real menu.
  useLayoutEffect(() => {
    const el = root.current;
    if (!el) return;
    const fit = () =>
      void getCurrentWindow().setSize(new LogicalSize(WIDTH, Math.ceil(el.scrollHeight)));
    fit();
    const ro = new ResizeObserver(fit);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const switchTo = async (id: string) => {
    setBusyId(id);
    try {
      const { snapshot: s, running_sessions: n } = await api.setActiveAccount(id);
      setSnapshot(s);
      setNotice(
        n > 0
          ? `Switched. ${n} running session${n === 1 ? "" : "s"} follow within ~30 s.`
          : "Switched. New claude sessions use this account.",
      );
    } catch (e) {
      setNotice(`Couldn't switch: ${String(e)}`);
    } finally {
      setBusyId(null);
    }
  };

  const active = snapshot?.accounts.find((a) => a.id === snapshot.active_id) ?? null;
  const others = snapshot?.accounts.filter((a) => a.id !== snapshot.active_id) ?? [];

  return (
    <div ref={root} className="pb-1.5 pt-2">
      {!snapshot ? (
        <p className="n-dim px-3.5 py-3">Loading…</p>
      ) : snapshot.accounts.length === 0 ? (
        <p className="n-dim px-3.5 py-2">No accounts yet. Open Clyde to add one.</p>
      ) : (
        <>
          {active ? <ActiveAccount account={active} /> : <NoActive />}
          {notice && <p className="n-dim px-3.5 pb-1 pt-0.5 text-[11px]">{notice}</p>}

          {others.length > 0 && (
            <>
              <div className="n-sep" />
              <div className="n-section">Other accounts</div>
              {others.map((a) => (
                <OtherAccount
                  key={a.id}
                  account={a}
                  busy={busyId === a.id}
                  disabled={busyId !== null}
                  onUse={() => switchTo(a.id)}
                />
              ))}
            </>
          )}
        </>
      )}

      <div className="n-sep" />
      <MenuRow onClick={() => invoke("open_main_window")}>
        <span className="flex-1">Browser tools</span>
        <span className="n-dim text-[12px]">
          {browser?.ready ? (browser.connected ? "Ready" : "Chrome closed") : "Set up…"}
        </span>
      </MenuRow>
      <MenuRow onClick={() => invoke("open_main_window")}>
        <span className="flex-1">Open Clyde…</span>
      </MenuRow>
      <MenuRow onClick={() => invoke("quit_app")}>
        <span className="flex-1">Quit Clyde</span>
        <span className="n-dim text-[12px]">⌘Q</span>
      </MenuRow>
    </div>
  );
}

// ---- active account ------------------------------------------------------

function ActiveAccount({ account }: { account: AccountView }) {
  const u = account.usage;
  const limited = u.status === "rejected";

  return (
    <div className="flex flex-col gap-2 px-3.5">
      <div className="flex items-center gap-2.5">
        <Avatar label={account.label} active />
        <div className="min-w-0 flex-1">
          <div className="truncate font-semibold">{account.label}</div>
          {account.email && account.email !== account.label && (
            <div className="n-dim truncate text-[11px]">{account.email}</div>
          )}
        </div>
        {account.subscription_type && (
          <span className="n-dim shrink-0 text-[11px]">{account.subscription_type}</span>
        )}
      </div>

      {limited && (
        <p className="text-[12px]" style={{ color: "var(--n-danger)" }}>
          Limit reached{u.resets_at ? ` · ${shortReset(u.resets_at)}` : ""}
        </p>
      )}
      {account.usage_error && (
        <p className="text-[12px]" style={{ color: "var(--n-warn)" }}>
          {account.usage_error}
        </p>
      )}

      <div className="flex flex-col gap-1.5 pb-1">
        {limitRows(account).map((r) => (
          <LimitRow key={r.label} {...r} />
        ))}
      </div>

      <p className="n-dim -mt-0.5 text-[11px]">
        {u.resets_at ? `Next reset in ${shortReset(u.resets_at).replace("resets in ", "")} · ` : ""}
        updated {ago(u.updated_at)}
      </p>
    </div>
  );
}

function NoActive() {
  return (
    <p className="n-dim px-3.5 py-1">
      Claude Code is signed into an account Clyde doesn't manage.
    </p>
  );
}

// ---- other accounts ------------------------------------------------------

function OtherAccount({
  account,
  busy,
  disabled,
  onUse,
}: {
  account: AccountView;
  busy: boolean;
  disabled: boolean;
  onUse: () => void;
}) {
  const worst = tightest(account);
  const limited = account.usage.status === "rejected";

  return (
    <div className="group flex items-center gap-2.5 px-3.5 py-1">
      <Avatar label={account.label} />
      <div className="min-w-0 flex-1">
        <div className="truncate">{account.label}</div>
        <div className="truncate text-[11px]">
          <AccountSummary account={account} />
        </div>
      </div>
      {!limited && worst != null && !account.usage_error && (
        <span className="text-[11px] tabular-nums group-hover:hidden" style={{ color: toneColor(worst) }}>
          {Math.round(worst)}%
        </span>
      )}
      {!loginBroken(account) && (
        <button
          className={`n-button ${busy ? "" : "hidden group-hover:inline-block"}`}
          disabled={disabled}
          onClick={onUse}
        >
          {busy ? "Switching…" : "Use"}
        </button>
      )}
    </div>
  );
}

// ---- bits ----------------------------------------------------------------

function MenuRow({ children, onClick }: { children: React.ReactNode; onClick: () => void }) {
  return (
    <div className="n-menu-row" onClick={onClick}>
      {children}
    </div>
  );
}
