import type { FC } from 'react';

const CAPABILITIES = ['routing', 'data', 'caching', 'bundling', 'serving', 'deployment'] as const;
const ADAPTERS = [
  { id: 'react',  label: 'React',  src: '/frameworks/react.png'  },
  { id: 'vue',    label: 'Vue',    src: '/frameworks/vue.png'    },
  { id: 'svelte', label: 'Svelte', src: '/frameworks/svelte.png' },
  { id: 'solid',  label: 'Solid',  src: '/frameworks/solid.png'  },
];

export const FrameworkAdapters: FC = () => (
  <section className="py-24 bg-[radial-gradient(circle,rgba(250,250,250,0.05)_1px,transparent_1px)] bg-[size:24px_24px] [mask-image:radial-gradient(ellipse_78%_62%_at_50%_50%,#000_25%,transparent_92%)]">
    <div className="max-w-4xl mx-auto grid gap-12 px-4">

      <div className="relative grid justify-items-center gap-4 p-8 sm:p-12 border border-zinc-800 rounded-xl bg-gradient-to-b from-zinc-500/10 to-transparent overflow-hidden">
        <div className="absolute top-0 left-0 w-full h-[1px] bg-gradient-to-r from-transparent via-zinc-400/30 to-transparent" />
        <span className="font-mono text-[10px] tracking-[0.28em] uppercase text-zinc-400">SHARED ENGINE</span>
        <h2 className="m-0 font-mono text-5xl font-medium tracking-tight text-zinc-50 leading-none">Rust Core</h2>
        <ul className="flex flex-wrap justify-center gap-x-3 gap-y-1 mt-4 p-0 list-none font-mono text-[11px] tracking-widest uppercase text-zinc-400">
          {CAPABILITIES.map((c, i) => (
            <li key={c} className="inline-flex items-baseline gap-3">
              {c}
              {i < CAPABILITIES.length - 1 && <span className="text-zinc-600">·</span>}
            </li>
          ))}
        </ul>
      </div>

      <ul className="grid grid-cols-2 sm:grid-cols-4 gap-6 list-none p-0 m-0">
        {ADAPTERS.map(a => (
          <li key={a.id} className="group grid justify-items-center gap-4">
            <span className="relative grid place-items-center w-[4.25rem] h-[4.25rem] rounded-full bg-zinc-200 shadow-[inset_0_1px_0_rgba(255,255,255,0.55),0_0_0_1px_rgba(250,250,250,0.1),0_0_24px_-6px_rgba(228,86,49,0.22),0_6px_18px_-8px_rgba(0,0,0,0.55)] transition-all duration-300 group-hover:-translate-y-1 group-hover:shadow-[inset_0_1px_0_rgba(255,255,255,0.8),0_0_0_1px_rgba(228,86,49,0.55),0_0_32px_-4px_rgba(228,86,49,0.45),0_10px_24px_-8px_rgba(0,0,0,0.7)]">
              <img src={a.src} alt="" width={36} height={36} loading="lazy" className="relative z-10 w-9 h-9 object-contain transition-transform duration-300 group-hover:scale-105" />
            </span>
            <span className="font-mono text-xs tracking-wider text-zinc-400 transition-colors group-hover:text-zinc-50">{a.label}</span>
          </li>
        ))}
      </ul>

      <p className="inline-flex items-baseline gap-4 justify-self-start m-0 font-mono text-[11px] tracking-widest uppercase text-zinc-400">
        <span className="text-zinc-600 tracking-[0.22em]">FIG. 01</span>
        <span>One engine. Four adapters. Identical by design.</span>
      </p>

    </div>
  </section>
);
