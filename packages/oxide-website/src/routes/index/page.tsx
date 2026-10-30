// src/routes/index/page.tsx — the landing page.
import { Island } from '@oxide-fw/react'
import Stats from '../../islands/stats.tsx'
import CopyInstall from '../../islands/copy-install.tsx'
import AdapterTabs from '../../islands/adapter-tabs.tsx'
import { site, stats } from '../../lib/site.ts'
import { FrameworkAdapters } from '../../components/FrameworkAdapters.tsx'

interface Props {
  data?: { latest?: { slug: string; title: string; date: string; description: string }[] }
}

const features = [
  { ix: '01', h: 'One engine, four frameworks', p: 'React, Vue, Svelte and Solid are first-class adapters over one shared engine — routing, data, caching, bundling, serving and deployment behave identically on all four. One conformance suite gates releases: an adapter drifts, the release blocks.' },
  { ix: '02', h: 'Fast paths that skip JavaScript', p: 'The route table is a build-time radix tree serialized to an rkyv archive and memory-mapped at boot. Redirects, rewrites, headers, basic auth and rate limits evaluate natively in Rust — the config path itself never evaluates JavaScript.' },
  { ix: '03', h: 'One wire format', p: 'ODF is a devalue-compatible binary format encoded natively in Rust and decoded by a single-file, zero-dependency TypeScript module. 16 golden fixtures + 8 encoder goldens lock it byte-exactly on both sides — Date, Map, Set and BigInt survive a cache round trip.' },
  { ix: '04', h: 'A single binary', p: 'hyper 1.x + rustls: HTTP/1 + HTTP/2 with HTTP/3 over QUIC and automatic Alt-Svc. Precompressed static serving, range requests, streaming responses with dynamic compression and Server-Timing. Everything that must run JavaScript sits behind one JsHost trait — V8 isolate pool or Node worker pool.' },
  { ix: '05', h: 'Deploy anywhere', p: 'Twelve deploy targets — Binary, Node, Static, Docker, Cloudflare, Cloudflare Pages, Lambda, Vercel, Vercel Functions, Netlify, Deno, Bun — behind a capability matrix that refuses at build time any configuration the target cannot honor.' },
  { ix: '06', h: 'Security and observability built in', p: 'CSP and CSRF modes, SSRF allow-lists, token-bucket rate limits, ChaCha20-Poly1305 session cookies with key rotation, ODF decode hardening with prototype-pollution guards. Prometheus metrics, W3C trace-context propagation, opt-in OTLP export.' },
]

const ADAPTERS = [
  { id: 'react', label: 'React', note: 'Server Components, Suspense and streaming SSR via react-dom/server.', snippet: '// src/routes/blog/[slug]/page.tsx\nexport async function load({ params }) {\n  return { post: await getPost(params.slug) }\n}\n\nexport default function Post({ data }) {\n  return <article>{data.post.title}</article>\n}' },
  { id: 'vue', label: 'Vue', note: 'Suspense and streaming via @vue/server-renderer.', snippet: '<!-- src/routes/blog/[slug]/page.vue -->\n<script setup lang="ts">\ndefineProps<{ data: { post: { title: string } } }>()\n</script>\n\n<template>\n  <article>{{ data.post.title }}</article>\n</template>' },
  { id: 'svelte', label: 'Svelte', note: 'Streaming SSR via svelte/server.', snippet: '<!-- src/routes/blog/[slug]/page.svelte -->\n<script lang="ts">\n  export let data: { post: { title: string } }\n</script>\n\n<article>{data.post.title}</article>' },
  { id: 'solid', label: 'Solid', note: 'Streaming SSR via @solidjs/web, native Suspense boundaries.', snippet: '// src/routes/blog/[slug]/page.tsx\nexport async function load({ params }) {\n  return { post: await getPost(params.slug) }\n}\n\nexport default function Post(props) {\n  return <article>{props.data.post.title}</article>\n}' }
];

export default function Page(props: Props) {
  const latest = props.data?.latest ?? []
  return (
    <main>
      <section className="hero">
        <div className="container">
          <h1>One Rust core.<br />Every framework.</h1>
          <p className="sub">{site.description}</p>
          <div className="cta">
            <a className="btn btn-primary" href="/docs">Get started</a>
            <a className="btn btn-ghost" href={site.repo} rel="noopener">View source ↗</a>
          </div>
          <div className="install-wrap">
            <Island component="CopyInstall" chunk="CopyInstall" strategy="visible" props={{ command: 'pnpm create oxide my-app' }}>
              <CopyInstall command="pnpm create oxide my-app" />
            </Island>
            <p className="install-note">
              Or from source: <code>git clone https://github.com/elcoosp/oxide</code>{' '}
              then <code>cargo build --release -p oxide-fw-cli</code>
            </p>
          </div>
        </div>
      </section>

      <FrameworkAdapters />

      <section className="stats">
        <div className="container">
          <Island component="Stats" chunk="Stats" strategy="visible" props={{ items: [...stats] }}>
            <Stats items={[...stats]} />
          </Island>
        </div>
      </section>

      <section className="section">
        <div className="container">
          <h2>What the hot path does without JavaScript</h2>
          <p className="lede">
            The request pipeline is ordered so cached, redirected, rewritten and static answers
            never enter a JS runtime. Streaming SSR is the primary live render path — real
            framework output, chunked as it resolves.
          </p>
          <div className="terminal">
            <div className="bar"><i></i><i></i><i></i></div>
            <pre>request → hyper 1.x → limits + request id
          → static assets (precompressed zstd › brotli › gzip, ETag, range)
          → native rules (redirects · rewrites · headers · auth · rate limits)
          → route match (rkyv archive, memory-mapped)
          → L0 response cache (weak-ETag 304s · stale-while-revalidate · single-flight)
          → JS middleware → loaders → dispatch: render | action | endpoint | server fn
          → streaming response (real SSR chunks) + Server-Timing
          → cache fill + tag invalidation</pre>
          </div>
        </div>
      </section>

      <section className="section">
        <div className="container">
          <h2>Features</h2>
          <p className="lede">
            Everything below ships today behind tests — over a thousand Rust test functions, a full TypeScript suite, and and a conformance gate that runs on all three platforms on every release
          </p>
          <div className="flex flex-col w-full max-w-6xl mx-auto py-4" data-reveal>
            {features.map((f) => (
              <article key={f.ix} className="grid grid-cols-[3rem_1fr] sm:grid-cols-[5rem_1fr] gap-4 sm:gap-8 py-8 border-t border-zinc-800 last:border-b">
                <span className="font-mono text-xs tracking-widest text-zinc-500 pt-1 select-none" aria-hidden="true">{f.ix}</span>
                <div className="flex flex-col gap-3">
                  <h3 className="text-2xl sm:text-3xl font-medium tracking-tight text-zinc-50 m-0">{f.h}</h3>
                  <p className="text-base leading-relaxed text-zinc-400 max-w-[68ch] m-0">{f.p}</p>
                </div>
              </article>
            ))}
          </div>
        </div>
      </section>

      <section className="section">
        <div className="container">
          <h2>Adapters</h2>
          <p className="lede">
            Rendering, hydration and HMR strategy are the only things an adapter owns. Everything
            else ships identically to every framework from the core runtime.
          </p>
          <div className="table-wrap">
            <table>
              <thead>
                <tr><th>Adapter</th><th>Package</th><th>Extensions</th><th>Suspense</th><th>HMR</th></tr>
              </thead>
              <tbody>
                <tr><td>React</td><td><code>@oxide-fw/react</code></td><td><code>.tsx</code></td><td>native</td><td>refresh</td></tr>
                <tr><td>Vue</td><td><code>@oxide-fw/vue</code></td><td><code>.vue</code></td><td>core</td><td>component</td></tr>
                <tr><td>Svelte</td><td><code>@oxide-fw/svelte</code></td><td><code>.svelte</code></td><td>core</td><td>component</td></tr>
                <tr><td>Solid</td><td><code>@oxide-fw/solid</code></td><td><code>.tsx</code></td><td>native</td><td>component</td></tr>
              </tbody>
            </table>
          </div>
        </div>
        <div className="adapter-code">
          <h3>What a route looks like</h3>
          <p className="lede">The adapter owns rendering, hydration and HMR strategy — nothing else differs. Same loader, same data shape, four component dialects.</p>
          <Island component="AdapterTabs" chunk="AdapterTabs" strategy="visible" props={{ adapters: ADAPTERS }}>
            <AdapterTabs adapters={ADAPTERS} />
          </Island>
        </div>
      </section>

      <section className="section">
        <div className="container">
          <div className="callout">
            <div className="t">Honest status</div>
            <p>Oxide is pre-alpha software built against a published specification — and audited against it: a 21-section compliance scorecard currently at <strong>95 / 100</strong>, with every finding public, including the ones we are not proud of. If you want production-hardened today, wait; if you want to watch a framework grow with its test suite open, welcome.</p>
            <p style={{ marginTop: '10px' }}><a href="/blog/auditing-our-own-framework">Read the audit post →</a></p>
          </div>
        </div>
      </section>

      <section className="section">
        <div className="container">
          <h2>From the blog</h2>
          <p className="lede"><a href="/blog">All posts →</a></p>
          <div className="posts">
            {latest.map((post) => (
              <a key={post.slug} className="post-card" href={`/blog/${post.slug}`}>
                <div className="meta"><span>{post.date}</span></div>
                <h3>{post.title}</h3>
                <p>{post.description}</p>
              </a>
            ))}
          </div>
        </div>
      </section>
    </main>
  )
}
