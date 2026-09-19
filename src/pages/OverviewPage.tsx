import { useEffect, useRef, useState } from "react";
import { Check, MoreHorizontal, Pencil, Trash2, X } from "lucide-react";
import { api } from "../lib/api";
import type { AccountView, AppSnapshot } from "../lib/types";
import { ChromeChip } from "../components/ChromeChip";
import {
  AccountSummary,
  Avatar,
  Group,
  GroupRow,
  LimitRow,
  ago,
  limitRows,
  loginBroken,
  shortReset,
  tightest,
} from "../ui/kit";

interface Props {
  snapshot: AppSnapshot;
  setSnapshot: (s: AppSnapshot) => void;
  browserReady: boolean;
  onOpenBrowserTools: () => void;
  onAdd: () => void;
}

/** Accounts: the active one in full, the rest with headroom and a switch. */
export function OverviewPage({ snapshot, setSnapshot, browserReady, onOpenBrowserTools, onAdd }: Props) {
  const [busyId, setBusyId] = useState<string | null>(null);
  const [notice, setNotice] = useState<{ tone: "ok" | "warn" | "danger"; text: string } | null>(null);

  const active = snapshot.accounts.find((a) => a.id === snapshot.active_id) ?? null;
  const others = snapshot.accounts.filter((a) => a.id !== snapshot.active_id);

  const switchTo = async (id: string) => {
    setBusyId(id);
    setNotice(null);
    try {
      const { snapshot: s, running_sessions: n } = await api.setActiveAccount(id);
      setSnapshot(s);
      setNotice({
        tone: "ok",
        text:
          n > 0
            ? `Switched. ${n} running claude session${n === 1 ? "" : "s"} follow within ~30 seconds.` +
              (browserReady ? " Browser tools keep working." : "")
            : "Switched. New claude sessions use this account.",
      });
    } catch (e) {
      setNotice({ tone: "danger", text: `Couldn't switch: ${String(e)}` });
    } finally {
      setBusyId(null);
    }
  };

  const rename = async (id: string, label: string) => setSnapshot(await api.renameAccount(id, label));
  const remove = async (id: string) => setSnapshot(await api.removeAccount(id));

  // Most headroom first; limited accounts, then ones we can't read, last.
  const rank = (a: AccountView) =>
    a.usage_error ? 300 : a.usage.status === "rejected" ? 200 + (tightest(a) ?? 0) : (tightest(a) ?? 150);
  const ranked = [...others].sort((a, b) => rank(a) - rank(b));
  const best = ranked.find((a) => !a.usage_error && a.usage.status !== "rejected" && tightest(a) != null);

  return (
    <div className="flex max-w-[640px] flex-col gap-6">
      {!browserReady && (
        <div className="-mx-4">
          <ChromeChip
            snapshot={snapshot}
            onActivate={switchTo}
            onOpenBrowserTools={onOpenBrowserTools}
            busyId={busyId}
          />
        </div>
      )}

      {notice && (
        <p
          className="rounded-lg px-3 py-2 text-[12px]"
          style={{
            color: `var(--n-${notice.tone})`,
            background: `color-mix(in srgb, var(--n-${notice.tone}) 12%, transparent)`,
          }}
        >
          {notice.text}
        </p>
      )}

      {active ? (
        <Group title="Active in Claude Code">
          <div className="flex flex-col gap-4 p-4">
            <AccountHeader account={active} onRename={rename} onRemove={remove} large />
            {active.usage.status === "rejected" && (
              <p className="text-[12px]" style={{ color: "var(--n-danger)" }}>
                Limit reached{active.usage.resets_at ? ` · ${shortReset(active.usage.resets_at)}` : ""}
              </p>
            )}
            {active.usage_error && (
              <p className="text-[12px]" style={{ color: "var(--n-warn)" }}>
                {active.usage_error}
              </p>
            )}
            <div className="grid grid-cols-1 gap-3.5 sm:grid-cols-2">
              {limitRows(active).map((r) => (
                <LimitRow key={r.label} {...r} large />
              ))}
            </div>
            <p className="n-dim text-[11px]">Updated {ago(active.usage.updated_at)}</p>
          </div>
        </Group>
      ) : (
        <Group title="Active in Claude Code">
          <GroupRow>
            <span className="n-dim">Claude Code is signed into an account Clyde doesn't manage. Add it to see its usage.</span>
          </GroupRow>
        </Group>
      )}

      {others.length > 0 && (
        <section className="flex flex-col gap-1.5">
          <h3 className="px-1 text-[12px] font-semibold text-[var(--n-text-2)]">
            Other accounts · most headroom first
          </h3>
          <div className="flex flex-col gap-2.5">
            {ranked.map((a) => (
              <div key={a.id} className="n-group flex flex-col gap-3 p-3.5">
                <div className="flex items-center gap-2">
                  <AccountHeader account={a} onRename={rename} onRemove={remove} />
                  {a.id === best?.id && (
                    <span
                      className="shrink-0 rounded-md px-1.5 py-0.5 text-[11px] font-medium"
                      style={{ color: "var(--n-ok)", background: "color-mix(in srgb, var(--n-ok) 14%, transparent)" }}
                    >
                      Most headroom
                    </span>
                  )}
                  {loginBroken(a) ? (
                    <button className="n-plain-button" onClick={onAdd} title="Its saved login stopped working">
                      Sign in again…
                    </button>
                  ) : (
                    <button className="n-plain-button" disabled={busyId !== null} onClick={() => switchTo(a.id)}>
                      {busyId === a.id ? "Switching…" : "Use"}
                    </button>
                  )}
                </div>
                {!a.usage_error && a.usage.updated_at > 0 && (
                  <>
                    <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                      {limitRows(a).map((r) => (
                        <LimitRow key={r.label} {...r} />
                      ))}
                    </div>
                    <p className="n-dim text-[11px]">
                      {a.subscription_type ? `${a.subscription_type} · ` : ""}updated {ago(a.usage.updated_at)}
                    </p>
                  </>
                )}
              </div>
            ))}
          </div>
        </section>
      )}

      <Group>
        <GroupRow>
          <span className="flex-1 text-[13px]">Add another account</span>
          <button className="n-plain-button" onClick={onAdd}>
            Add…
          </button>
        </GroupRow>
      </Group>
    </div>
  );
}

function AccountHeader({
  account,
  onRename,
  onRemove,
  large = false,
}: {
  account: AccountView;
  onRename: (id: string, label: string) => void;
  onRemove: (id: string) => void;
  large?: boolean;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(account.label);
  const [menu, setMenu] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  // Close like a native menu: on any click outside it, or Escape.
  useEffect(() => {
    if (!menu) return;
    const close = () => {
      setMenu(false);
      setConfirm(false);
    };
    const onDown = (e: MouseEvent) => {
      if (!menuRef.current?.contains(e.target as Node)) close();
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && close();
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [menu]);

  return (
    <div className="flex min-w-0 flex-1 items-center gap-3">
      <Avatar label={account.label} active={account.is_active} size={large ? 36 : 28} />
      <div className="min-w-0 flex-1">
        {editing ? (
          <form
            className="flex items-center gap-1"
            onSubmit={(e) => {
              e.preventDefault();
              onRename(account.id, draft.trim() || account.label);
              setEditing(false);
            }}
          >
            <input
              autoFocus
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              className="min-w-0 flex-1 rounded-md border border-[var(--n-card-border)] bg-[var(--n-window)] px-2 py-0.5 text-[13px] outline-none focus:ring-2 focus:ring-[var(--n-accent)]"
            />
            <button type="submit" className="p-1 text-[var(--n-accent)]" aria-label="Save name">
              <Check size={14} />
            </button>
            <button type="button" className="p-1 n-dim" aria-label="Cancel" onClick={() => setEditing(false)}>
              <X size={14} />
            </button>
          </form>
        ) : (
          <div className={`truncate ${large ? "text-[15px] font-semibold" : "text-[13px]"}`}>{account.label}</div>
        )}
        <div className="truncate text-[11px]">
          {large ? (
            <span className="n-dim">
              {[account.email !== account.label ? account.email : null, account.subscription_type]
                .filter(Boolean)
                .join(" · ")}
            </span>
          ) : account.usage_error || account.usage.status === "rejected" || !account.usage.updated_at ? (
            <AccountSummary account={account} />
          ) : (
            <span className="n-dim">Tightest limit {Math.round(tightest(account) ?? 0)}%</span>
          )}
        </div>
      </div>

      <div className="relative" ref={menuRef}>
        <button className="rounded-md p-1 n-dim hover:bg-[var(--n-hover)]" aria-label="More" onClick={() => setMenu((m) => !m)}>
          <MoreHorizontal size={15} />
        </button>
        {menu && (
          <div className="card absolute right-0 top-7 z-20 flex w-40 flex-col p-1 text-[13px]">
            <button
              className="n-menu-row !mx-0 text-left"
              onClick={() => {
                setEditing(true);
                setMenu(false);
              }}
            >
              <Pencil size={13} /> Rename
            </button>
            {confirm ? (
              <button className="n-menu-row !mx-0 text-left" style={{ color: "var(--n-danger)" }} onClick={() => onRemove(account.id)}>
                <Trash2 size={13} /> Confirm remove
              </button>
            ) : (
              <button className="n-menu-row !mx-0 text-left" onClick={() => setConfirm(true)}>
                <Trash2 size={13} /> Remove…
              </button>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
