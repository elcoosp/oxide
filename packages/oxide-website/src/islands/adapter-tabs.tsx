import { useState } from 'react';
import { codeToHtml } from 'rangi';
import { githubDark } from 'rangi/themes';


// rangi emits <pre style="margin:10px 0; padding:30px 20px; background:#0d1117; ...">
// Inline styles beat every class rule. Strip the style attr in the HTML
// string so our CSS owns layout completely.
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



function stripShikiStyle(html: string): string {
  // rangi v2.2.0 emits <div class="shj-lang-tsx shj-multiline" style="...">
  // with layout baked in (margin:10px 0; padding:30px 20px; font:18px Consolas).
  // Strip style="" from any <div> that carries a shj-* class. Inner <span style="color:...">
  // tokens keep their colors — those are what we want.
  return html.replace(/<div\b([^>]*)>/g, (match, attrs) => {
    if (/class="[^"]*shj-/.test(attrs)) {
      return `<div${attrs.replace(/\s+style="[^"]*"/, '')}>`;
    }
    return match;
  });
}

export default function AdapterTabs({ adapters }: Props) {
  const [activeId, setActiveId] = useState(adapters[0]?.id);
  const current = adapters.find(a => a.id === activeId) ?? adapters[0];

  if (!current) return null;

  const lang = current.id === 'solid' ? 'tsx'
             : current.id === 'vue' ? 'vue'
             : current.id === 'svelte' ? 'svelte'
             : 'tsx';

  // Note: rangi is a shiki-compatible highlighter. It emits the same
  // `.shiki` class names and inline `style="color:..."` attributes on tokens,
  // so any existing shiki CSS keeps working. The `.shiki` class in the DOM
  // is intentional — that's how rangi ships.
  const highlightedHtml = stripShikiStyle(codeToHtml(current.snippet, {
    lang,
    theme: githubDark,
    lineNumbers: false,
  }));

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
