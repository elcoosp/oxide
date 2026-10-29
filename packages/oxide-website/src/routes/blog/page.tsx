// src/routes/blog/index/page.tsx — blog index.
interface Props {
  data?: {
    posts?: { slug: string; title: string; date: string; description: string; tags: string[] }[]
  }
}

export default function Page(props: Props) {
  const posts = props.data?.posts ?? []
  return (
    <main className="container">
      <section className="post-header">
        <div className="meta">
          <span>blog</span>
          <span>{posts.length} post{posts.length === 1 ? '' : 's'}</span>
        </div>
        <h1>Notes from building Oxide</h1>
        <p style={{ color: 'var(--muted)', margin: 0, maxWidth: '46em' }}>
          Architecture deep-dives, honest status reports, and the occasional numbers post. Every
          claim here is backed by a test in the repo.
        </p>
      </section>
      <div className="posts" style={{ paddingBottom: 24 }}>
        {posts.map((p) => (
          <a className="post-card" href={`/blog/${p.slug}`} key={p.slug}>
            <div className="meta">
              <span>{p.date}</span>
              {p.tags.map((t) => (
                <span className="tag" key={t}>
                  #{t}
                </span>
              ))}
            </div>
            <h3>{p.title}</h3>
            <p>{p.description}</p>
          </a>
        ))}
      </div>
    </main>
  )
}
