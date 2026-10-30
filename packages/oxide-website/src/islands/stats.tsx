import { TestTube, Layers, Workflow, Rocket } from 'lucide-react';


/// Deterministic 1,000-style separator. `Number.toLocaleString()` uses the
/// runtime's Intl locale data — Node emits "1,251", some browsers emit
/// "1 251" (narrow no-break space). React sees different DOM strings on
/// server vs client and throws hydration #418. This regex formatter is
/// identical everywhere.
function formatNumber(n: number): string {
  return String(n).replace(/\B(?=(\d{3})+(?!\d))/g, ',');
}


interface Stat {
  label: string;
  value: number;
}

interface Props {
  items: Stat[];
}

// Map label → icon. If the label ever changes, fallback is Layers.
const ICONS: Record<string, typeof TestTube> = {
  'Rust test functions':    TestTube,
  'framework adapters':     Layers,
  'CI jobs, all platforms': Workflow,
  'deploy targets':         Rocket,
};

export default function Stats({ items }: Props) {
  return (
    <div className="grid grid-cols-2 md:grid-cols-4 gap-4 md:gap-5 mt-12">
      {items.map((s) => {
        const Icon = ICONS[s.label] ?? Layers;
        return (
          <div
            key={s.label}
            className="group relative flex flex-col justify-between gap-4 p-5 bg-zinc-900/40 border border-zinc-800 rounded-[10px] transition-colors duration-200 hover:border-zinc-700 hover:bg-zinc-900/60 overflow-hidden"
          >
            {/* corner accent — a single 40px diagonal mark, not a stripe */}
            <div
              className="pointer-events-none absolute -top-px -right-px w-10 h-10 opacity-30 group-hover:opacity-60 transition-opacity"
              style={{
                background:
                  'linear-gradient(225deg, var(--color-accent) 0%, var(--color-accent) 1px, transparent 1px)',
              }}
            />

            {/* Icon in a mono-tinted tile */}
            <div className="flex items-start justify-between">
              <span
                className="inline-flex w-8 h-8 items-center justify-center rounded-md bg-zinc-800/70 text-zinc-400 group-hover:text-[var(--color-accent)] transition-colors"
              >
                <Icon size={16} strokeWidth={1.75} />
              </span>
            </div>

            {/* Value — mono, large, accent */}
            <div>
              <div className="font-mono text-3xl md:text-[2rem] font-medium tabular-nums leading-none text-[var(--color-accent)] tracking-tight">
                {formatNumber(s.value)}
              </div>
              <div className="mt-2 font-mono text-[11px] uppercase tracking-wider text-zinc-500">
                {s.label}
              </div>
            </div>
          </div>
        );
      })}
    </div>
  );
}
