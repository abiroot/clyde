import { useEffect, useState } from "react";
import { Copy, Check } from "lucide-react";
import { api } from "../lib/api";
import type { AppSnapshot, RoutingView, Session, Target } from "../lib/types";
import { Group, GroupRow } from "../ui/kit";

const TARGET_LABEL: Record<Target, string> = { claude: "Claude", kimi: "Kimi", refuseless: "Refuseless" };
const COMMAND: Record<Target, string> = {
  claude: "claude-anthropic",
  kimi: "claude-kimi",
  refuseless: "claude-refuseless",
};

/** Running `claude` sessions, refreshed every few seconds while visible, with
 *  per-folder pins and the commands that start a session on a set target. */
export function SessionsPage({ snapshot }: { snapshot: AppSnapshot }) {
  const [sessions, setSessions] = useState<Session[] | null>(null);
  const [routing, setRouting] = useState<RoutingView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    const load = () => api.listSessions().then((s) => live && setSessions(s));
    load();
    api.getRouting().then((r) => live && setRouting(r));
    const t = setInterval(load, 4000);
    return () => {
      live = false;
      clearInterval(t);
    };
  }, []);

  const connected = snapshot.accounts.filter((a) => a.provider !== "claude").map((a) => a.provider as Target);
  const targets: Target[] = ["claude", ...connected];
  const globalName =
    snapshot.accounts.find((a) => a.id === snapshot.active_id)?.provider === "claude"
      ? "Claude"
      : (snapshot.accounts.find((a) => a.id === snapshot.active_id)?.label ?? "Claude");

  const home = sessions
    ?.map((s) => s.cwd?.match(/^\/Users\/[^/]+/)?.[0])
    .find((h): h is string => !!h);
  const tidy = (p: string) => (home && p.startsWith(home) ? "~" + p.slice(home.length) : p);

  const pin = async (folder: string, target: string) => {
    setError(null);
    try {
      setRouting(await api.setFolderPin(folder, target === "" ? null : (target as Target)));
    } catch (e) {
      setError(String(e));
    }
  };

  const install = async () => {
    setError(null);
    try {
      setRouting(await api.installCommands());
    } catch (e) {
      setError(String(e));
    }
  };

  const copy = async (text: string) => {
    await api.copyText(text);
    setCopied(text);
    setTimeout(() => setCopied(null), 1500);
  };

  // Folders pinned but with no session running right now still deserve a row.
  const idlePins = Object.entries(routing?.folder_pins ?? {}).filter(
    ([f]) => !sessions?.some((s) => s.cwd === f),
  );

  return (
    <div className="flex max-w-[640px] flex-col gap-6">
      {error && (
        <p className="rounded-lg px-3 py-2 text-[12px]" style={{ color: "var(--n-danger)", background: "color-mix(in srgb, var(--n-danger) 12%, transparent)" }}>
          {error}
        </p>
      )}

      <Group
        title={sessions ? `${sessions.length} running` : "Running"}
        footer={`Unpinned sessions follow Clyde's choice (now ${globalName}), including automatic switches. A folder pin applies within seconds to every session in that folder and to new ones there; it lives in the folder's .claude/settings.local.json. Sessions started with their own CLAUDE_CONFIG_DIR keep their own login.`}
      >
        {!sessions ? (
          <GroupRow>
            <span className="n-dim">Looking…</span>
          </GroupRow>
        ) : sessions.length === 0 && idlePins.length === 0 ? (
          <GroupRow>
            <span className="n-dim">No claude sessions are running.</span>
          </GroupRow>
        ) : (
          <>
            {sessions.map((s) => {
              const folder = s.cwd ? (s.cwd.split("/").filter(Boolean).pop() ?? "/") : "Unknown folder";
              const pinned = s.cwd ? routing?.folder_pins[s.cwd] : undefined;
              return (
                <GroupRow key={s.pid}>
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-[13px]">{folder}</div>
                    <div className="n-dim truncate text-[11px]">{s.cwd ? tidy(s.cwd) : `pid ${s.pid}`}</div>
                  </div>
                  {s.config_dir ? (
                    <span className="rounded-md bg-[var(--n-track)] px-1.5 py-0.5 text-[11px] n-dim" title={s.config_dir}>
                      own login
                    </span>
                  ) : s.pin ? (
                    <span className="rounded-md bg-[var(--n-track)] px-1.5 py-0.5 text-[11px] n-dim" title="Started with a launch command">
                      {TARGET_LABEL[s.pin]} · launch
                    </span>
                  ) : (
                    s.cwd && (
                      <PinSelect value={pinned ?? ""} targets={targets} globalName={globalName} onChange={(t) => pin(s.cwd!, t)} />
                    )
                  )}
                  <span className="n-dim w-16 text-right text-[12px] tabular-nums">{uptime(s.running_secs)}</span>
                </GroupRow>
              );
            })}
            {idlePins.map(([f, t]) => (
              <GroupRow key={f}>
                <div className="min-w-0 flex-1">
                  <div className="truncate text-[13px]">{f.split("/").filter(Boolean).pop()}</div>
                  <div className="n-dim truncate text-[11px]">{tidy(f)} · no session running</div>
                </div>
                <PinSelect value={t} targets={targets} globalName={globalName} onChange={(v) => pin(f, v)} />
                <span className="w-16" />
              </GroupRow>
            ))}
          </>
        )}
      </Group>

      <Group
        title="Start a session on a set target"
        footer="These start one session on that target whatever Clyde's choice, and auto-switch leaves it there. Add --model <id> or use /model inside to pick that session's model. They also work in cmux."
      >
        {targets.map((t) => {
          const cmd = routing?.commands_installed ? COMMAND[t] : `claude --settings ~/.claude/clyde/${t}.json`;
          return (
            <GroupRow key={t}>
              <span className="w-20 shrink-0 text-[13px]">{TARGET_LABEL[t]}</span>
              <code className="min-w-0 flex-1 truncate font-mono text-[12px]">{cmd}</code>
              <button className="n-plain-button" onClick={() => copy(cmd)} aria-label={`Copy ${cmd}`}>
                {copied === cmd ? <Check size={13} /> : <Copy size={13} />}
              </button>
            </GroupRow>
          );
        })}
        {!routing?.commands_installed && (
          <GroupRow>
            <span className="flex-1 text-[12px] n-dim">Shorter: install claude-anthropic, claude-kimi and claude-refuseless in ~/.local/bin.</span>
            <button className="n-plain-button" onClick={install}>
              Install
            </button>
          </GroupRow>
        )}
      </Group>
    </div>
  );
}

function PinSelect({
  value,
  targets,
  globalName,
  onChange,
}: {
  value: string;
  targets: Target[];
  globalName: string;
  onChange: (target: string) => void;
}) {
  return (
    <select
      aria-label="Folder runs on"
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className="max-w-[150px] rounded-md border border-[var(--n-card-border)] bg-[var(--n-window)] px-2 py-0.5 text-[12px]"
      title="Where every session in this folder runs"
    >
      <option value="">Follow Clyde ({globalName})</option>
      {targets.map((t) => (
        <option key={t} value={t}>
          Pin: {TARGET_LABEL[t]}
        </option>
      ))}
    </select>
  );
}

function uptime(secs: number | null): string {
  if (secs == null) return "";
  if (secs < 60) return "just now";
  const m = Math.floor(secs / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  return h < 24 ? `${h}h ${m % 60}m` : `${Math.floor(h / 24)}d ${h % 24}h`;
}
