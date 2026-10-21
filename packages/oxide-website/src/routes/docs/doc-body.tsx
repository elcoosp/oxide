// src/routes/docs/doc-body.tsx — shared docs page body (used by the
// /docs index and /docs/[page]; both live under the docs layout).
interface Doc {
  meta: { title: string; description: string }
  html: string
}

export default function DocBody({ doc }: { doc?: Doc }) {
  if (!doc) return null
  return (
    <article>
      <div className="breadcrumb">docs / {doc.meta.title.toLowerCase()}</div>
      <h1 style={{ fontSize: 34, letterSpacing: '-1px', margin: '0 0 10px' }}>{doc.meta.title}</h1>
      <p style={{ color: 'var(--muted)', margin: '0 0 8px' }}>{doc.meta.description}</p>
      {/* html comes from our own markdown renderer over repo-owned files */}
      <div className="prose" dangerouslySetInnerHTML={{ __html: doc.html }} />
    </article>
  )
}
