import type { FC } from 'react';

const ADAPTERS = {
  react:  { label: 'React',  src: '/frameworks/react.png'  },
  vue:    { label: 'Vue',    src: '/frameworks/vue.png'    },
  svelte: { label: 'Svelte', src: '/frameworks/svelte.png' },
  solid:  { label: 'Solid',  src: '/frameworks/solid.png'  },
};

export const FrameworkAdapters: FC = () => {
  return (
    <section className="relative py-32 overflow-hidden bg-[#09090b]">
      {/* Background ambient glow */}
      <div className="absolute inset-0 flex items-center justify-center pointer-events-none">
        <div className="w-[600px] h-[600px] bg-[#e45631] rounded-full blur-[150px] opacity-[0.04] mix-blend-screen" />
      </div>

      <div className="relative max-w-5xl mx-auto px-6 flex flex-col items-center">

        {/* ORBITAL SCENE */}
        <div className="orbit-container relative w-[300px] h-[300px] sm:w-[500px] sm:h-[500px] flex items-center justify-center my-4 cursor-crosshair">

          {/* CENTRAL REACTOR CORE */}
          <div className="absolute z-10 flex items-center justify-center w-24 h-24 sm:w-32 sm:h-32 rounded-full bg-zinc-900 border border-zinc-700 shadow-[0_0_80px_rgba(228,86,49,0.3)]">
            <div className="absolute inset-0 rounded-full bg-[#e45631]/20 animate-ping-slow" />
            <div className="relative z-10 w-16 h-16 sm:w-20 sm:h-20 bg-white rounded-full flex items-center justify-center shadow-[inset_0_-5px_15px_rgba(0,0,0,0.3)] border border-white/50">
              <img src="/logo.png" alt="Oxide Core" className="w-10 h-10 sm:w-14 sm:h-14 object-contain contrast-125" />
            </div>
          </div>

          {/* INNER ORBIT (React, Svelte) */}
          <div className="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 w-[180px] h-[180px] sm:w-[260px] sm:h-[260px] rounded-full border border-zinc-800 animate-spin-slow">

            {/* React (Top) */}
            <div className="absolute top-0 left-1/2 -translate-x-1/2 -translate-y-1/2 animate-spin-slow-reverse flex flex-col items-center gap-2 group">
              <div className="orbit-logo-tile"><img src={ADAPTERS.react.src} alt="React" /></div>
              <span className="orbit-label">{ADAPTERS.react.label}</span>
            </div>

            {/* Svelte (Bottom) */}
            <div className="absolute bottom-0 left-1/2 -translate-x-1/2 translate-y-1/2 animate-spin-slow-reverse flex flex-col items-center gap-2 group">
              <div className="orbit-logo-tile"><img src={ADAPTERS.svelte.src} alt="Svelte" /></div>
              <span className="orbit-label">{ADAPTERS.svelte.label}</span>
            </div>
          </div>

          {/* OUTER ORBIT (Vue, Solid) */}
          <div className="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 w-[280px] h-[280px] sm:w-[420px] sm:h-[420px] rounded-full border border-zinc-800 border-dashed animate-spin-slow-reverse">

            {/* Vue (Left) */}
            <div className="absolute top-1/2 left-0 -translate-x-1/2 -translate-y-1/2 animate-spin-slow flex flex-col items-center gap-2 group">
              <div className="orbit-logo-tile"><img src={ADAPTERS.vue.src} alt="Vue" /></div>
              <span className="orbit-label">{ADAPTERS.vue.label}</span>
            </div>

            {/* Solid (Right) */}
            <div className="absolute top-1/2 right-0 translate-x-1/2 -translate-y-1/2 animate-spin-slow flex flex-col items-center gap-2 group">
              <div className="orbit-logo-tile"><img src={ADAPTERS.solid.src} alt="Solid" /></div>
              <span className="orbit-label">{ADAPTERS.solid.label}</span>
            </div>
          </div>

        </div>

        <p className="mt-12 font-mono text-[11px] tracking-[0.2em] uppercase text-zinc-500 flex items-center gap-4 relative z-20">
          <span className="text-[#e45631]">◉</span>
          Identical by design.
        </p>
      </div>
    </section>
  );
};
