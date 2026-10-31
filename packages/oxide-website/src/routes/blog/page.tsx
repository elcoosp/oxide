// src/routes/blog/page.tsx
interface Props {
  data?: {
    posts?: { slug: string; title: string; date: string; description: string; tags: string[] }[]
  }
}

export default function Page(props: Props) {
  const posts = props.data?.posts ?? []
  return (
    <div className="max-w-4xl mx-auto px-6 py-32">
      <section className="mb-16">
        <div className="flex items-center gap-4 font-mono text-xs tracking-widest uppercase text-zinc-500 mb-4">
          <span>blog</span>
          <span>{posts.length} post{posts.length === 1 ? '' : 's'}</span>
        </div>
        <h1 className="text-5xl md:text-7xl font-bold tracking-tighter mb-6 text-zinc-50">Notes from building Oxide</h1>
        <p className="text-zinc-400 text-lg max-w-2xl leading-relaxed">
          Architecture deep-dives, honest status reports, and the occasional numbers post. Every
          claim here is backed by a test in the repo.
        </p>
      </section>
      <div className="flex flex-col gap-6">
        {posts.map((p) => (
          <a className="group block border border-zinc-800 rounded-xl p-8 hover:border-zinc-600 hover:bg-zinc-900/30 transition-all" href={`/blog/${p.slug}`} key={p.slug}>
            <div className="flex flex-wrap items-center gap-3 font-mono text-xs text-zinc-500 mb-4">
              <span>{p.date}</span>
              {p.tags.map((t) => (
                <span className="bg-zinc-800/80 text-zinc-300 px-2.5 py-1 rounded-md" key={t}>
                  #{t}
                </span>
              ))}
            </div>
            <h3 className="text-2xl font-semibold mb-3 tracking-tight text-zinc-50 group-hover:text-white">{p.title}</h3>
            <p className="text-zinc-400 leading-relaxed">{p.description}</p>
          </a>
        ))}
      </div>
    </div>
  )
}
