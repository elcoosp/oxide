// src/routes/rss/route.ts — RSS 2.0 feed for the blog (§8.1 endpoints:
// route.ts exports per-method handlers; no JS runtime needed on hits that
// cache). Point newsreaders and newsletter editors at /rss.
import { listPosts, xmlEsc } from '../../lib/md.server.ts'
import { site } from '../../lib/site.ts'

export async function GET(_request: Request): Promise<Response> {
  const items = (await listPosts())
    // A post without a parseable `date` would emit `<pubDate>Invalid Date
    // </pubDate>` (invalid RSS — some readers drop the whole feed); skip it
    // here. `listPosts` already sorts unparseable dates last.
    .filter((p) => !Number.isNaN(Date.parse(p.date)))
    .map(
      (p) => `    <item>
      <title>${xmlEsc(p.title)}</title>
      <link>${xmlEsc(`${site.url}/blog/${p.slug}`)}</link>
      <guid isPermaLink="true">${xmlEsc(`${site.url}/blog/${p.slug}`)}</guid>
      <pubDate>${new Date(p.date).toUTCString()}</pubDate>
      <description>${xmlEsc(p.description)}</description>
    </item>`,
    )
    .join('\n')

  const xml = `<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>${xmlEsc(site.name)} — blog</title>
    <link>${site.url}/blog</link>
    <description>${xmlEsc(site.description)}</description>
    <language>en</language>
${items}
  </channel>
</rss>`

  return new Response(xml, {
    headers: { 'content-type': 'application/rss+xml; charset=utf-8' },
  })
}
