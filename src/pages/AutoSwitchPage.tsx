import type { PointerEvent, ReactNode } from "react";
import { useEffect, useRef, useState } from "react";
import { ChevronDown, GripVertical } from "lucide-react";
import { api } from "../lib/api";
import type { AccountView, AppSnapshot, AutoSwitchSettings, Rule, Settings } from "../lib/types";
import { Avatar, Group, GroupRow, Segmented, Toggle, ago, loginBroken, scopedLabel, tightest, toneColor } from "../ui/kit";

/** Thresholds offered in the pickers; 100 reads as "full". */
const LEVELS = [50, 60, 70, 75, 80, 85, 90, 95, 100];

/** Mirrors `autoswitch::HEADROOM`: room an account needs before Clyde moves to it. */
const HEADROOM = 5;

/**
 * Auto-switch: when the active account uses up the limit its rule watches,
 * Claude Code moves to the next account in the priority list that has room —
 * and back again once a preferred account resets. Accounts can be kept out of
 * rotation or given their own rule.
 */
export function AutoSwitchPage({ snapshot }: { snapshot: AppSnapshot }) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [autostart, setAutostart] = useState(true);

  useEffect(() => {
    api.getSettings().then(setSettings).catch((e) => setError(String(e)));
    api.getAutostart().then(setAutostart).catch(() => {});
  }, []);

  const save = async (patch: Partial<AutoSwitchSettings>) => {
    if (!settings) return;
    const next = { ...settings, auto_switch: { ...settings.auto_switch, ...patch } };
    setSettings(next);
    try {
      setSettings(await api.setSettings(next));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  };

  if (!settings) {
    return (
      <div className="flex max-w-[640px] flex-col gap-6">
        <div className="n-group h-[110px] animate-pulse" />
        <div className="n-group h-[160px] animate-pulse" />
      </div>
    );
  }

  const cfg = settings.auto_switch;
  const order = ordered(cfg, snapshot.accounts);
  const limits = limitChoices(snapshot.accounts);
  const name = (id: string) => {
    const a = snapshot.accounts.find((x) => x.id === id);
    return a?.email ?? a?.label ?? "a removed account";
  };

  const setPolicy = (id: string, patch: { exclude?: boolean; rule?: Rule | null }) => {
    const cur = cfg.accounts[id] ?? { exclude: false, rule: null };
    save({ accounts: { ...cfg.accounts, [id]: { ...cur, ...patch } } });
  };

  return (
    <div className="flex max-w-[640px] flex-col gap-6">
      <Group
        footer={
          snapshot.last_switch ? (
            <>
              Last switch: {name(snapshot.last_switch.from)} → {name(snapshot.last_switch.to)},{" "}
              {ago(snapshot.last_switch.at)} ({snapshot.last_switch.reason}). Running sessions follow within ~30 seconds.
            </>
          ) : (
            "Clyde checks every 2 minutes. Running claude sessions follow a switch within ~30 seconds."
          )
        }
      >
        <GroupRow>
          <span className="flex-1">
            Switch accounts automatically
            <span className="n-dim block text-[11px]">When the active account reaches its limit, move to the next one with room</span>
          </span>
          <Toggle label="Auto-switch" checked={cfg.enabled} onChange={(v) => save({ enabled: v })} />
        </GroupRow>
        {cfg.enabled && (
          <>
            {!autostart && (
              <GroupRow>
                <span className="flex-1 text-[12px]" style={{ color: "var(--n-warn)" }}>
                  Clyde only switches while it's running.
                </span>
                <button
                  className="n-plain-button"
                  onClick={() => api.setAutostart(true).then(setAutostart).catch((e) => setError(String(e)))}
                >
                  Open Clyde at login
                </button>
              </GroupRow>
            )}
            <GroupRow>
              <span className="flex-1">
                Go back to a preferred account when it resets
                <span className="n-dim block text-[11px]">Not after you pick an account yourself, until that one runs out</span>
              </span>
              <Toggle label="Switch back" checked={cfg.switch_back} onChange={(v) => save({ switch_back: v })} />
            </GroupRow>
            <GroupRow>
              <span className="flex-1">Notify me when Clyde switches</span>
              <Toggle label="Switch notifications" checked={cfg.notify} onChange={(v) => save({ notify: v })} />
            </GroupRow>
          </>
        )}
      </Group>

      <Group title="Default rule" footer="Every account follows this unless it has its own rule below.">
        <GroupRow>
          <span className="flex-1">Switch away when</span>
          <RulePicker rule={cfg.default_rule} limits={limits} onChange={(r) => save({ default_rule: r })} />
        </GroupRow>
      </Group>

      <Group
        title="Priority"
        footer="Drag to reorder. Clyde always uses the highest account that still has room."
      >
        <PriorityList
          order={order}
          onReorder={(ids) => save({ priority: ids })}
          render={(a, index, handle) => {
            const policy = cfg.accounts[a.id];
            const rule = policy?.rule ?? cfg.default_rule;
            const expanded = open === a.id;
            return (
              <div className="flex flex-col">
                <div className="flex items-center gap-2.5">
                  {handle}
                  <span className="n-dim w-4 text-right text-[12px] tabular-nums">{index + 1}</span>
                  <Avatar label={a.email ?? a.label} active={a.is_active} size={22} />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate">{a.email ?? a.label}</span>
                    <span className="n-dim block truncate text-[11px]">
                      {policy?.exclude ? "Never switched to" : policy?.rule ? describe(policy.rule) : "Default rule"}
                    </span>
                  </span>
                  <StateChip account={a} rule={rule} excluded={!!policy?.exclude} />
                  <button
                    aria-label={`Options for ${a.email ?? a.label}`}
                    aria-expanded={expanded}
                    onClick={() => setOpen(expanded ? null : a.id)}
                    className="rounded-md p-1 hover:bg-[var(--n-hover)]"
                  >
                    <ChevronDown
                      size={14}
                      className="transition-transform"
                      style={{ transform: expanded ? "rotate(180deg)" : undefined }}
                    />
                  </button>
                </div>
                {expanded && (
                  <div className="ml-[58px] mt-2.5 flex flex-col gap-2.5 border-t border-[var(--n-sep)] pt-2.5">
                    <div className="flex items-center gap-2">
                      <span className="flex-1 text-[12px]">Never switch to this account automatically</span>
                      <Toggle
                        label="Exclude from auto-switch"
                        checked={!!policy?.exclude}
                        onChange={(v) => setPolicy(a.id, { exclude: v })}
                      />
                    </div>
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="flex-1 text-[12px]">Switch away from it when</span>
                      <Segmented
                        value={policy?.rule ? "custom" : "default"}
                        options={[
                          { value: "default", label: "Default rule" },
                          { value: "custom", label: "Its own rule" },
                        ]}
                        onChange={(v) => setPolicy(a.id, { rule: v === "custom" ? { ...cfg.default_rule } : null })}
                      />
                    </div>
                    {policy?.rule && (
                      <div className="flex justify-end">
                        <RulePicker rule={policy.rule} limits={limits} onChange={(r) => setPolicy(a.id, { rule: r })} />
                      </div>
                    )}
                  </div>
                )}
              </div>
            );
          }}
        />
      </Group>

      {error && (
        <p className="px-1 text-[12px]" style={{ color: "var(--n-danger)" }}>
          {error}
        </p>
      )}
    </div>
  );
}

// ---- pieces ------------------------------------------------------------------

function RulePicker({ rule, limits, onChange }: { rule: Rule; limits: { value: string; label: string }[]; onChange: (r: Rule) => void }) {
  const select = "rounded-md border border-[var(--n-card-border)] bg-[var(--n-window)] px-2 py-0.5 text-[13px]";
  // Keep a model cap that no account reports right now selectable.
  const options = limits.some((l) => l.value === rule.limit) ? limits : [...limits, { value: rule.limit, label: scopedLabel(rule.limit) }];
  return (
    <div className="flex items-center gap-1.5">
      <select aria-label="Limit" value={rule.limit} onChange={(e) => onChange({ ...rule, limit: e.target.value })} className={select}>
        {options.map((l) => (
          <option key={l.value} value={l.value}>
            {l.label}
          </option>
        ))}
      </select>
      <span className="n-dim text-[12px]">reaches</span>
      <select aria-label="Threshold" value={rule.at} onChange={(e) => onChange({ ...rule, at: Number(e.target.value) })} className={select}>
        {(LEVELS.includes(rule.at) ? LEVELS : [...LEVELS, rule.at].sort((a, b) => a - b)).map((t) => (
          <option key={t} value={t}>
            {t === 100 ? "100% (full)" : `${t}%`}
          </option>
        ))}
      </select>
    </div>
  );
}

/** Where an account stands against its rule, in one word. */
function StateChip({ account, rule, excluded }: { account: AccountView; rule: Rule; excluded: boolean }) {
  const p = reading(account, rule);
  let text: string;
  let color: string;
  if (loginBroken(account)) {
    text = "Signed out";
    color = "var(--n-danger)";
  } else if (account.usage_error || account.usage.updated_at === 0) {
    text = "No reading";
    color = "var(--n-text-3)";
  } else if (account.usage.status === "rejected" || (p != null && p >= rule.at)) {
    text = "Used up";
    color = "var(--n-danger)";
  } else if (account.is_active) {
    text = p == null ? "In use" : `In use · ${Math.round(p)}%`;
    color = "var(--n-accent)";
  } else if (excluded) {
    text = "Skipped";
    color = "var(--n-text-3)";
  } else {
    const room = p == null || p < rule.at - HEADROOM;
    text = p == null ? "Has room" : `${Math.round(p)}%`;
    color = room ? toneColor(p) : "var(--n-warn)";
  }
  return (
    <span className="shrink-0 text-[12px] font-medium tabular-nums" style={{ color }}>
      {text}
    </span>
  );
}

/**
 * A list reordered by dragging its handles. Pointer events rather than HTML5
 * drag and drop, which the webview's file-drop handling can swallow.
 */
function PriorityList({
  order,
  onReorder,
  render,
}: {
  order: AccountView[];
  onReorder: (ids: string[]) => void;
  render: (a: AccountView, index: number, handle: ReactNode) => ReactNode;
}) {
  const [draft, setDraft] = useState<string[] | null>(null);
  const [dragging, setDragging] = useState<string | null>(null);
  const rows = useRef(new Map<string, HTMLDivElement>());

  const ids = draft ?? order.map((a) => a.id);
  const byId = new Map(order.map((a) => [a.id, a]));

  const onMove = (e: PointerEvent) => {
    if (!dragging || !draft) return;
    // The row whose vertical middle the pointer has passed.
    let target = draft.length - 1;
    for (let i = 0; i < draft.length; i++) {
      const r = rows.current.get(draft[i])?.getBoundingClientRect();
      if (r && e.clientY < r.top + r.height / 2) {
        target = i;
        break;
      }
    }
    const from = draft.indexOf(dragging);
    if (from === target) return;
    const next = draft.filter((id) => id !== dragging);
    next.splice(target, 0, dragging);
    setDraft(next);
  };

  const end = () => {
    if (draft && draft.join() !== order.map((a) => a.id).join()) onReorder(draft);
    setDragging(null);
    setDraft(null);
  };

  if (order.length === 0) {
    return (
      <GroupRow>
        <span className="n-dim">Add accounts to set their order.</span>
      </GroupRow>
    );
  }

  return (
    <div onPointerMove={onMove} onPointerUp={end} onPointerCancel={end}>
      {ids.map((id, i) => {
        const a = byId.get(id);
        if (!a) return null;
        const handle = (
          <span
            aria-label="Drag to reorder"
            role="button"
            tabIndex={0}
            onPointerDown={(e) => {
              (e.target as HTMLElement).setPointerCapture?.(e.pointerId);
              setDragging(id);
              setDraft(order.map((x) => x.id));
            }}
            onKeyDown={(e) => {
              // Keyboard: ↑/↓ move the row.
              const d = e.key === "ArrowUp" ? -1 : e.key === "ArrowDown" ? 1 : 0;
              if (!d) return;
              e.preventDefault();
              const cur = order.map((x) => x.id);
              const j = i + d;
              if (j < 0 || j >= cur.length) return;
              [cur[i], cur[j]] = [cur[j], cur[i]];
              onReorder(cur);
            }}
            className="n-dim cursor-grab touch-none select-none rounded p-0.5 hover:text-[var(--n-text)] active:cursor-grabbing"
          >
            <GripVertical size={14} />
          </span>
        );
        return (
          <div
            key={id}
            ref={(el) => {
              if (el) rows.current.set(id, el);
              else rows.current.delete(id);
            }}
            className="n-group-row !block"
            style={dragging === id ? { background: "var(--n-selected)" } : undefined}
          >
            {render(a, i, handle)}
          </div>
        );
      })}
    </div>
  );
}

// ---- logic (mirrors src-tauri/src/autoswitch.rs) ---------------------------------

function ordered(cfg: AutoSwitchSettings, accounts: AccountView[]): AccountView[] {
  const listed = cfg.priority.map((id) => accounts.find((a) => a.id === id)).filter((a): a is AccountView => !!a);
  return [...listed, ...accounts.filter((a) => !cfg.priority.includes(a.id))];
}

function reading(a: AccountView, rule: Rule): number | null {
  const u = a.usage;
  switch (rule.limit) {
    case "any":
      return tightest(a);
    case "session":
      return u.five_hour_utilization;
    case "weekly":
      return u.seven_day_utilization;
    default:
      return u.scoped_limits?.find((l) => l.label === rule.limit)?.percent ?? null;
  }
}

function limitName(limit: string): string {
  if (limit === "any") return "Any limit";
  if (limit === "session") return "Session";
  if (limit === "weekly") return "Weekly";
  return scopedLabel(limit);
}

function describe(rule: Rule): string {
  return rule.at >= 100 ? `${limitName(rule.limit)} full` : `${limitName(rule.limit)} ≥ ${rule.at}%`;
}

/** Any, session, weekly, then every model cap some account reports. */
function limitChoices(accounts: AccountView[]): { value: string; label: string }[] {
  const scoped = new Set<string>();
  for (const a of accounts) for (const l of a.usage.scoped_limits ?? []) scoped.add(l.label);
  return [
    { value: "any", label: "Any limit" },
    { value: "session", label: "Session (5 hours)" },
    { value: "weekly", label: "Weekly" },
    ...[...scoped].sort().map((l) => ({ value: l, label: scopedLabel(l) })),
  ];
}
