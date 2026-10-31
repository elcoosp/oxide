// src/routes/docs/doc-body.tsx
import AdapterTabs from '../../islands/adapter-tabs.tsx'
import type { MdBlock } from '../../lib/md.server.ts'

interface Doc {
  meta: { title: string; description: string }
  html: MdBlock[]
}

const META = {
  react: { label: 'React', logo: '/frameworks/react.png', note: 'React 19 adapter — streaming SSR + concurrent features.' },
  vue: { label: 'Vue', logo: '/frameworks/vue.png', note: 'Vue 3 adapter — SFC support + Vapor runtime.' },
  svelte: { label: 'Svelte', logo: '/frameworks/svelte.png', note: 'Svelte 5 adapter — runes + server components.' },
  solid: { label: 'Solid', logo: '/frameworks/solid.png', note: 'Solid adapter — fine-grained reactivity.' }
} as const

export default function DocBody({ doc }: { doc?: Doc }) {
  if (!doc) return null

  return (
    <article>
      <div className="breadcrumb">docs / {doc.meta.title.toLowerCase()}</div>
      <h1 style={{ fontSize: 34, letterSpacing: '-1px', margin: '0 0 10px' }}>{doc.meta.title}fez</h1>
      <p style={{ color: 'var(--muted)', margin: '0 0 8px' }}>{doc.meta.description}</p>

      <div className="prose">
        {doc.html.map((block, i) => {
          if (typeof block === 'string') {
            return <div key={i} dangerouslySetInnerHTML={{ __html: block }} />
          }
          if (block.type === 'framework-tabs') {
            const adapters = (['react', 'vue', 'svelte', 'solid'] as const).map(id => ({
              id,
              label: META[id].label,
              logo: META[id].logo,
              note: META[id].note,
              snippet: block.data[id] || ''
            }))
            return (
              <div key={i} className="not-prose my-8">
                <AdapterTabs adapters={adapters} />
              </div>
            )
          }
          return null
        })}
      </div>
    </article>
  )
}
// touch 1793459638426574000
// touch 1793459893515450000
// touch 1793460510036586000
// touch 1793460861160267000
