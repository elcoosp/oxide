import { useState, useEffect } from 'react';
import type { Highlighter } from 'shiki';

interface Adapter { id: string; label: string; note: string; snippet: string; }
interface Props { adapters: Adapter[]; }

let highlighterPromise: Promise<Highlighter> | null = null;
function getHighlighter() {
  if (!highlighterPromise) {
    highlighterPromise = import('shiki').then(m =>
      m.createHighlighter({
        themes: ['github-dark-dimmed'],
        langs: ['tsx', 'vue', 'svelte', 'js']
      })
    );
  }
  return highlighterPromise;
}

function escapeHtml(s: string) {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
          .replace(/"/g, '&quot;').replace(/'/g, '&#039;');
}

export default function AdapterTabs({ adapters }: Props) {
  const [activeId, setActiveId] = useState(adapters[0]?.id);
  const [html, setHtml] = useState('');

  const current = adapters.find(a => a.id === activeId) ?? adapters[0];

  useEffect(() => {
    let cancelled = false;
    if (!current) return;

    // 1. Initial fallback (Tailwind classes)
    setHtml(`<pre class="m-0 p-5 bg-[#22272e] rounded-[10px] overflow-x-auto font-mono text-[13px] leading-relaxed"><code class="bg-transparent">${escapeHtml(current.snippet)}</code></pre>`);

    // 2. Shiki with Tailwind classes injected
    getHighlighter().then(h => {
      if (cancelled) return;
      const lang = current.id === 'solid' ? 'tsx' : current.id;
      const safeLang = ['tsx', 'vue', 'svelte', 'js'].includes(lang) ? lang : 'tsx';
      const out = h.codeToHtml(current.snippet, { lang: safeLang, theme: 'github-dark-dimmed' });

      const patched = out.replace(
        /<pre class="shiki"/,
        `<pre class="shiki m-0 p-5 rounded-[10px] overflow-x-auto font-mono text-[13px] leading-relaxed"`
      );
      setHtml(patched);
    }).catch(e => console.warn('Shiki failed:', e));

    return () => { cancelled = true; };
  }, [current]);

  if (!current) return null;

  return (
    <div className="max-w-[1080px] mx-auto border border-zinc-800 rounded-[10px] overflow-hidden">
      <div role="tablist" aria-label="Framework adapter" className="flex gap-0 p-1.5 bg-[#09090b] border-b border-zinc-800">
        {adapters.map(a => (
          <button
            key={a.id}
            type="button"
            role="tab"
            aria-selected={activeId === a.id}
            aria-controls={`panel-${a.id}`}
            tabIndex={activeId === a.id ? 0 : -1}
            onClick={() => setActiveId(a.id)}
            className={`flex-none w-max px-3.5 py-2 font-mono text-[13.5px] cursor-pointer border border-transparent rounded-[7px] transition-colors ${
              activeId === a.id ? 'bg-zinc-900 text-zinc-50' : 'bg-transparent text-zinc-400 hover:text-zinc-200'
            }`}
          >
            {a.label}
          </button>
        ))}
      </div>

      <div className="p-4 sm:p-5">
        <div dangerouslySetInnerHTML={{ __html: html }} />
        <p className="mt-4 mb-0 text-zinc-400 text-sm font-mono">{current.note}</p>
      </div>
    </div>
  );
}
