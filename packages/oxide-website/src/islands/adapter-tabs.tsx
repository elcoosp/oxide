import { useState } from 'react';
import { codeToHtml } from 'rangi';
import { githubDark } from 'rangi/themes';

interface Adapter {
  id: string;
  label: string;
  logo?: string;
  note: string;
  snippet: string;
}

interface Props {
  adapters: Adapter[];
}

export default function AdapterTabs({ adapters }: Props) {
  const [activeId, setActiveId] = useState(adapters[0]?.id);
  const current = adapters.find(a => a.id === activeId) ?? adapters[0];

  if (!current) return null;

  const lang = current.id === 'solid' ? 'tsx'
             : current.id === 'vue' ? 'vue'
             : current.id === 'svelte' ? 'svelte'
             : 'tsx';

  const highlightedHtml = codeToHtml(current.snippet, {
    lang,
    theme: githubDark,
  });

  return (
    <div className="max-w-[1080px] mx-auto border border-zinc-800 rounded-[10px] overflow-hidden">
      <div role="tablist" className="flex flex-wrap gap-1 p-1.5 bg-[#09090b] border-b border-zinc-800">
        {adapters.map(a => (
          <button
            key={a.id}
            type="button"
            role="tab"
            aria-selected={activeId === a.id}
            onClick={() => setActiveId(a.id)}
            className={`inline-flex items-center gap-2 px-3 py-2 font-mono text-[13px] cursor-pointer border border-transparent rounded-[7px] transition-colors ${
              activeId === a.id
                ? 'bg-zinc-900 text-zinc-50'
                : 'bg-transparent text-zinc-400 hover:text-zinc-200'
            }`}
          >
            {a.logo && (
              <span className="flex items-center justify-center w-[18px] h-[18px] rounded-full bg-zinc-200 overflow-hidden shrink-0">
                <img
                  src={a.logo}
                  alt=""
                  width={12}
                  height={12}
                  style={{ width: '12px', height: '12px', objectFit: 'contain' }}
                />
              </span>
            )}
            {a.label}
          </button>
        ))}
      </div>

      <div className="adapter-shiki bg-zinc-900">
        <div dangerouslySetInnerHTML={{ __html: highlightedHtml }} />
      </div>

      <p className="m-0 px-5 py-4 bg-[#09090b] text-zinc-400 text-sm font-mono border-t border-zinc-800">
        {current.note}
      </p>
    </div>
  );
}
