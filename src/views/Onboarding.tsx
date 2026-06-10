import type { ReactNode } from "react";
import { Plus, Shuffle, ShieldCheck, Gauge } from "lucide-react";

interface Props {
  onAdd: () => void;
}

export function Onboarding({ onAdd }: Props) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center px-6 pb-6 text-center">
      <div
        className="grid h-16 w-16 place-items-center rounded-2xl text-3xl font-bold"
        style={{ background: "var(--color-clay)", color: "#1a0f0a" }}
      >
        C
      </div>
      <h1 className="mt-4 text-lg font-semibold">Welcome to Clyde</h1>
      <p className="mt-1.5 max-w-[300px] text-sm leading-relaxed text-[var(--color-ink-soft)]">
        Run plain <code>claude</code> across all your Claude accounts. Clyde switches
        between them automatically — before you ever hit a limit.
      </p>

      <div className="mt-6 flex w-full max-w-[320px] flex-col gap-2.5 text-left">
        <Feature icon={<Shuffle size={15} />} title="One command, many accounts">
          Keep using <code>claude</code> with one set of settings.
        </Feature>
        <Feature icon={<Gauge size={15} />} title="Live usage, auto-failover">
          Routes to the freshest account and fails over on a limit.
        </Feature>
        <Feature icon={<ShieldCheck size={15} />} title="Tokens stay in your Keychain">
          Nothing is written to plaintext or sent anywhere but Anthropic.
        </Feature>
      </div>

      <button
        onClick={onAdd}
        className="no-drag mt-7 flex items-center gap-2 rounded-xl bg-[var(--color-clay)] px-5 py-2.5 text-sm font-semibold text-[#1a0f0a] hover:opacity-90"
      >
        <Plus size={16} /> Import your Claude accounts
      </button>
    </div>
  );
}

function Feature({
  icon,
  title,
  children,
}: {
  icon: ReactNode;
  title: string;
  children: ReactNode;
}) {
  return (
    <div className="card flex items-start gap-3 p-3">
      <div className="mt-0.5 text-[var(--color-clay)]">{icon}</div>
      <div className="leading-tight">
        <div className="text-xs font-medium">{title}</div>
        <div className="text-[11px] text-[var(--color-ink-faint)]">{children}</div>
      </div>
    </div>
  );
}
