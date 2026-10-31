// src/routes/blog/[slug]/page.tsx
import type { MdBlock } from '../../../lib/md.server.ts'

interface Props {
  data?: {
    post?: {
      meta: { slug: string; title: string; date: string; tags: string[]; description: string }
      html: MdBlock[]
    }
  }
}

export default function Page(props: Props) {
  const post = props.data?.post
  if (!post) return null
  return (
    <div className="max-w-3xl mx-auto px-6 py-32">
      <article>
        <header className="mb-14">
          <div className="flex flex-wrap items-center gap-3 font-mono text-xs text-zinc-500 mb-6">
            <span>{post.meta.date}</span>
            {post.meta.tags.map((t) => (
              <span className="bg-zinc-800/80 text-zinc-300 px-2.5 py-1 rounded-md" key={t}>
                #{t}
              </span>
            ))}
          </div>
          <h1 className="text-5xl font-bold tracking-tighter mb-6 text-zinc-50">{post.meta.title}</h1>
          <p className="text-zinc-400 text-lg leading-relaxed max-w-2xl">{post.meta.description}</p>
        </header>

        <div className="prose">
          {post.html.map((block, i) => {
            if (typeof block === 'string') {
              return <div key={i} dangerouslySetInnerHTML={{ __html: block }} />
            }
            if (block.type === 'framework-tabs') {
              // Blog posts don't use framework-tabs, but handle it safely
              return null
            }
            return null
          })}
        </div>
      </article>
    </div>
  )
}
