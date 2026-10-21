// src/routes/blog/[slug]/page.tsx — blog post renderer.
interface Props {
  data?: {
    post?: {
      meta: { slug: string; title: string; date: string; tags: string[]; description: string }
      html: string
    }
  }
}

export default function Page(props: Props) {
  const post = props.data?.post
  if (!post) return null
  return (
    <main className="container">
      <article>
        <header className="post-header">
          <div className="meta">
            <span>{post.meta.date}</span>
            {post.meta.tags.map((t) => (
              <span className="tag" key={t}>
                #{t}
              </span>
            ))}
          </div>
          <h1>{post.meta.title}</h1>
          <p style={{ color: 'var(--muted)', margin: 0 }}>{post.meta.description}</p>
        </header>
        {/* html comes from our own markdown renderer over repo-owned files */}
        <div className="prose" dangerouslySetInnerHTML={{ __html: post.html }} />
      </article>
    </main>
  )
}
