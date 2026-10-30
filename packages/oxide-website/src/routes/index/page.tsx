import { Island } from '@oxide-fw/react'
import CopyInstall from '../../islands/copy-install.tsx'
import AdapterTabs from '../../islands/adapter-tabs.tsx'
import { site } from '../../lib/site.ts'
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
  { id: 'react', label: 'React', logo: '/frameworks/react.png', note: 'Server Components, Suspense and streaming SSR via react-dom/server.', snippet: '// src/routes/blog/[slug]/page.tsx\nexport async function load({ params }) {\n  return { post: await getPost(params.slug) }\n}\n\nexport default function Post({ data }) {\n  return <article>{data.post.title}</article>\n}' },
  { id: 'vue', label: 'Vue', logo: '/frameworks/vue.png', note: 'Suspense and streaming via @vue/server-renderer.', snippet: '<!-- src/routes/blog/[slug]/page.vue -->\n<script setup lang="ts">\ndefineProps<{ data: { post: { title: string } } }>()\n</script>\n\n<template>\n  <article>{{ data.post.title }}</article>\n</template>' },
  { id: 'svelte', label: 'Svelte', logo: '/frameworks/svelte.png', note: 'Streaming SSR via svelte/server.', snippet: '<!-- src/routes/blog/[slug]/page.svelte -->\n<script lang="ts">\n  export let data: { post: { title: string } }\n</script>\n\n<article>{data.post.title}</article>' },
  { id: 'solid', label: 'Solid', logo: '/frameworks/solid.png', note: 'Streaming SSR via @solidjs/web, native Suspense boundaries.', snippet: '// src/routes/blog/[slug]/page.tsx\nexport async function load({ params }) {\n  return { post: await getPost(params.slug) }\n}\n\nexport default function Post(props) {\n  return <article>{props.data.post.title}</article>\n}' }
];

export default function Page(props: Props) {
  const latest = props.data?.latest ?? []
  return (
    <main>
      {/* HERO */}
      <section className="relative pt-32 pb-24 overflow-hidden">
        <div className="absolute inset-0 bg-[radial-gradient(ellipse_at_top,rgba(228,86,49,0.1),transparent_50%)] pointer-events-none" />
        <div className="max-w-6xl mx-auto px-6 relative z-10 grid grid-cols-1 lg:grid-cols-2 gap-16 items-center">
          <div>
            <h1 className="text-5xl md:text-6xl font-bold tracking-tight text-white mb-6 leading-[1.05]">
              One Rust core.<br />Every framework.
            </h1>
            <p className="text-lg text-zinc-400 max-w-lg mb-8 leading-relaxed">
              A framework-agnostic full-stack meta-framework with a Rust core: real streaming SSR for React, Vue, Svelte and Solid, served from a single static binary with HTTP/3 and a byte-parity cache.
            </p>
            <div className="flex flex-wrap gap-4">
              <a className="btn btn-primary" href="/docs">Get started</a>
              <a className="btn btn-ghost" href={site.repo} rel="noopener">View source ↗</a>
            </div>
          </div>
          <div className="relative">
            <div className="absolute -inset-1 bg-gradient-to-r from-zinc-800 to-zinc-700 rounded-xl blur opacity-20"></div>
            <div className="relative bg-[#0c0c0e] border border-zinc-800 rounded-xl shadow-2xl overflow-hidden">
              <div className="flex items-center px-4 py-3 bg-zinc-900 border-b border-zinc-800">
                <div className="flex gap-2">
                  <div className="w-3 h-3 rounded-full bg-zinc-700"></div>
                  <div className="w-3 h-3 rounded-full bg-zinc-700"></div>
                  <div className="w-3 h-3 rounded-full bg-zinc-700"></div>
                </div>
                <span className="ml-4 text-xs font-mono text-zinc-500">bash</span>
              </div>
              <div className="p-5 font-mono text-sm">
                <div className="flex items-center text-zinc-200 mb-6">
                  <span className="text-[#e45631] mr-3">$</span>
                  <Island component="CopyInstall" chunk="CopyInstall" strategy="visible" props={{ command: 'pnpm create oxide my-app' }}>
                    <CopyInstall command="pnpm create oxide my-app" />
                  </Island>
                </div>
                <div className="text-zinc-500 text-xs leading-relaxed">
                  Or from source: <br/>
                  <span className="text-zinc-400 select-all">git clone https://github.com/elcoosp/oxide</span> <br/>
                  <span className="text-zinc-400 select-all">cargo build --release -p oxide-fw-cli</span>
                </div>
              </div>
            </div>
          </div>
        </div>
      </section>

      <FrameworkAdapters />

      {/* PIPELINE */}
      <section className="section" data-reveal >
        <div className="container">
          <h2>What the hot path does without JavaScript</h2>
          <p className="lede">
            The request pipeline is ordered so cached, redirected, rewritten and static answers
            never enter a JS runtime. Streaming SSR is the primary live render path — real
            framework output, chunked as it resolves.
          </p>
          <div className="pipeline">
            {[
              { n: '01', t: 'Request',              d: 'hyper 1.x · limits · request id' },
              { n: '02', t: 'Static assets',        d: 'precompressed zstd › brotli › gzip · ETag · range' },
              { n: '03', t: 'Native rules',         d: 'redirects · rewrites · headers · auth · rate limits' },
              { n: '04', t: 'Route match',          d: 'rkyv archive · memory-mapped · zero-alloc' },
              { n: '05', t: 'L0 response cache',    d: 'weak-ETag 304s · stale-while-revalidate · single-flight' },
              { n: '06', t: 'JS middleware → loaders', d: 'dispatch: render | action | endpoint | server fn' },
              { n: '07', t: 'Streaming response',   d: 'real SSR chunks · Server-Timing header' },
              { n: '08', t: 'Cache fill',           d: 'tag-based invalidation' },
            ].map((step, i, arr) => (
              <div key={step.n} className="pipeline-step">
                <div className="pipeline-rail">
                  <span className="pipeline-dot" />
                  {i < arr.length - 1 && <span className="pipeline-line" />}
                </div>
                <div className="pipeline-content">
                  <div className="pipeline-header">
                    <span className="pipeline-num">{step.n}</span>
                    <span className="pipeline-title">{step.t}</span>
                  </div>
                  <p className="pipeline-detail">{step.d}</p>
                </div>
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* FEATURES */}
      <section className="section" data-reveal >
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

      {/* ADAPTERS + CODE */}
      <section className="section" data-reveal >
        <div className="container">
          <h2>Adapters</h2>
          <p className="lede">
            Rendering, hydration and HMR strategy are the only things an adapter owns. Everything
            else ships identically to every framework from the core runtime.
          </p>
          <div className="mt-8 overflow-x-auto rounded-xl border border-zinc-800 bg-zinc-900/30">
            <table className="w-full text-left text-sm min-w-[600px]">
              <thead className="bg-zinc-900/50 text-xs uppercase tracking-wider text-zinc-500 font-mono">
                <tr>
                  <th className="px-6 py-4 font-medium">Adapter</th>
                  <th className="px-6 py-4 font-medium">Package</th>
                  <th className="px-6 py-4 font-medium">Extensions</th>
                  <th className="px-6 py-4 font-medium">Suspense</th>
                  <th className="px-6 py-4 font-medium">HMR</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-zinc-800/50">
                {[
                  { name: 'React', pkg: '@oxide-fw/react', ext: '.tsx', sus: 'native', hmr: 'refresh', repo: 'react' },
                  { name: 'Vue', pkg: '@oxide-fw/vue', ext: '.vue', sus: 'core', hmr: 'component', repo: 'vue' },
                  { name: 'Svelte', pkg: '@oxide-fw/svelte', ext: '.svelte', sus: 'core', hmr: 'component', repo: 'svelte' },
                  { name: 'Solid', pkg: '@oxide-fw/solid', ext: '.tsx', sus: 'native', hmr: 'component', repo: 'solid' },
                ].map((row) => (
                  <tr key={row.name} className="transition-colors hover:bg-zinc-800/30">
                    <td className="px-6 py-4 font-medium text-zinc-200">
                      <a href={`https://github.com/elcoosp/oxide/tree/main/packages/${row.repo}`} className="hover:text-[#e45631] transition-colors" target="_blank" rel="noopener noreferrer">
                        {row.name}
                      </a>
                    </td>
                    <td className="px-6 py-4">
                      <a href={`https://www.npmjs.com/package/${row.pkg}`} className="font-mono text-xs text-zinc-400 hover:text-zinc-200 transition-colors bg-zinc-800/50 px-2 py-1 rounded-md" target="_blank" rel="noopener noreferrer">
                        {row.pkg}
                      </a>
                    </td>
                    <td className="px-6 py-4"><code className="text-xs text-[#e45631]">{row.ext}</code></td>
                    <td className="px-6 py-4 text-zinc-400">{row.sus}</td>
                    <td className="px-6 py-4 text-zinc-400">{row.hmr}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
        <div className="container mt-16">
          <div className="adapter-code">
            <h2>What a route looks like</h2>
            <p className="lede">The adapter owns rendering, hydration and HMR strategy — nothing else differs. Same loader, same data shape, four component dialects.</p>
            <Island component="AdapterTabs" chunk="AdapterTabs" strategy="visible" props={{ adapters: ADAPTERS }}>
              <AdapterTabs adapters={ADAPTERS} />
            </Island>
          </div>
        </div>
      </section>

      {/* BLOG */}
      <section className="section pb-32">
        <div className="container">
          <h2>From the blog</h2>
          <p className="lede"><a href="/blog">All posts →</a></p>
          <div className="grid grid-cols-1 md:grid-cols-3 gap-6">
            {latest.map((post) => (
              <a key={post.slug} href={`/blog/${post.slug}`} className="group flex flex-col justify-between p-6 bg-zinc-900/30 border border-zinc-800 rounded-xl hover:bg-zinc-900/60 hover:border-zinc-700 hover:-translate-y-1 hover:shadow-lg transition-all duration-300">
                <div>
                  <div className="font-mono text-xs text-zinc-500 mb-3">{post.date}</div>
                  <h3 className="text-lg font-medium text-zinc-200 mb-2 group-hover:text-[#e45631] transition-colors leading-snug">{post.title}</h3>
                  <p className="text-sm text-zinc-400 line-clamp-3 leading-relaxed">{post.description}</p>
                </div>
                <div className="mt-6 flex items-center text-xs font-mono tracking-wider uppercase text-zinc-500 group-hover:text-zinc-300 transition-colors">
                  Read Post <span className="ml-2 group-hover:translate-x-1 transition-transform duration-300">→</span>
                </div>
              </a>
            ))}
          </div>
        </div>
      </section>
    </main>
  )
}
