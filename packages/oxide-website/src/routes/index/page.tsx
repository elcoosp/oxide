// src/routes/index/page.tsx — the landing page.
import { Island } from '@oxide/react'
import Stats from '../../islands/stats.tsx'
import { site, stats } from '../../lib/site.ts'

interface Props {
  data?: { latest?: { slug: string; title: string; date: string; description: string }[] }
}

const features = [
  {
    ix: '01',
    h: 'One engine, four frameworks',
    p: 'React, Vue, Svelte and Solid are first-class adapters over one shared engine — routing, data, caching, bundling, serving and deployment behave identically on all four. One 24-assertion conformance suite gates releases: an adapter drifts, the release blocks.',
  },
  {
    ix: '02',
    h: 'Fast paths that skip JavaScript',
    p: 'The route table is a build-time radix tree serialized to an rkyv archive and memory-mapped at boot. Redirects, rewrites, headers, basic auth and rate limits evaluate natively in Rust — the config path itself never evaluates JavaScript.',
  },
  {
    ix: '03',
    h: 'One wire format',
    p: 'ODF is a devalue-compatible binary format encoded natively in Rust and decoded by a single-file, zero-dependency TypeScript module. 16 golden fixtures + 8 encoder goldens lock it byte-exactly on both sides — Date, Map, Set and BigInt survive a cache round trip.',
  },
  {
    ix: '04',
    h: 'A single binary',
    p: 'hyper 1.x + rustls: HTTP/1 + HTTP/2 with HTTP/3 over QUIC and automatic Alt-Svc. Precompressed static serving, range requests, streaming responses with dynamic compression and Server-Timing. Everything that must run JavaScript sits behind one JsHost trait — V8 isolate pool or Node worker pool.',
  },
  {
    ix: '05',
    h: 'Deploy anywhere',
    p: 'Twelve deploy targets — Binary, Node, Static, Docker, Cloudflare, Lambda, Vercel, Netlify, Deno, Bun and more — behind a capability matrix that refuses at build time any configuration the target cannot honor.',
  },
  {
    ix: '06',
    h: 'Security and observability built in',
    p: 'CSP and CSRF modes, SSRF allow-lists, token-bucket rate limits, ChaCha20-Poly1305 session cookies with key rotation, ODF decode hardening with prototype-pollution guards. Prometheus metrics, W3C trace-context propagation, opt-in OTLP export.',
  },
]

export default function Page(props: Props) {
  const latest = props.data?.latest ?? []
  return (
    <main>
      <section className="hero">
        <div className="container">
          <span className="kicker">v0.1 · pre-alpha, audited in public</span>
          <h1>
            One Rust core.
            <br />
            <span className="grad">Every framework.</span>
          </h1>
          <p className="sub">{site.description}</p>
          <div className="cta">
            <a className="btn btn-primary" href="/docs">
              Get started
            </a>
            <a className="btn btn-ghost" href={site.repo} rel="noopener">
              View source ↗
            </a>
          </div>
          <div className="install">
            <div>
              <span className="c"># build the single binary and scaffold an app</span>
            </div>
            <div>
              <span className="p">$</span> git clone https://github.com/elcoosp/oxide && cd oxide
            </div>
            <div>
              <span className="p">$</span> cargo build --release -p oxide-cli
            </div>
            <div>
              <span className="p">$</span> node packages/create-oxide/src/cli.ts blog react my-app
            </div>
          </div>
        </div>
      </section>

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
            <div className="bar">
              <i></i>
              <i></i>
              <i></i>
            </div>
            <pre>{`request → hyper 1.x → limits + request id
        → static assets (precompressed zstd › brotli › gzip, ETag, range)
        → native rules (redirects · rewrites · headers · auth · rate limits)
        → route match (rkyv archive, memory-mapped)
        → L0 response cache (weak-ETag 304s · stale-while-revalidate · single-flight)
        → JS middleware → loaders → dispatch: render | action | endpoint | server fn
        → streaming response (real SSR chunks) + Server-Timing
        → cache fill + tag invalidation`}</pre>
          </div>
        </div>
      </section>

      <section className="section">
        <div className="container">
          <h2>Features</h2>
          <p className="lede">
            Everything below ships today behind tests — 1,116 Rust test functions, 349 TypeScript
            cases, and a 96-check conformance gate run on every release.
          </p>
          <div className="features">
            {features.map((f) => (
              <div className="feature" key={f.ix}>
                <h3>
                  <span className="ix">{f.ix}</span>
                  {f.h}
                </h3>
                <p>{f.p}</p>
              </div>
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
                <tr>
                  <th>Adapter</th>
                  <th>Package</th>
                  <th>Extensions</th>
                  <th>Suspense</th>
                  <th>HMR</th>
                </tr>
              </thead>
              <tbody>
                <tr>
                  <td>React</td>
                  <td>
                    <code>@oxide/react</code>
                  </td>
                  <td>
                    <code>.tsx</code>
                  </td>
                  <td>native</td>
                  <td>refresh</td>
                </tr>
                <tr>
                  <td>Vue</td>
                  <td>
                    <code>@oxide/vue</code>
                  </td>
                  <td>
                    <code>.vue</code>
                  </td>
                  <td>core</td>
                  <td>component</td>
                </tr>
                <tr>
                  <td>Svelte</td>
                  <td>
                    <code>@oxide/svelte</code>
                  </td>
                  <td>
                    <code>.svelte</code>
                  </td>
                  <td>core</td>
                  <td>component</td>
                </tr>
                <tr>
                  <td>Solid</td>
                  <td>
                    <code>@oxide/solid</code>
                  </td>
                  <td>
                    <code>.tsx</code>
                  </td>
                  <td>native</td>
                  <td>component</td>
                </tr>
              </tbody>
            </table>
          </div>
        </div>
      </section>

      <section className="section">
        <div className="container">
          <div className="callout">
            <div className="t">Honest status</div>
            <p>
              Oxide is pre-alpha software built against a published specification — and audited
              against it: a 21-section compliance scorecard currently at{' '}
              <strong>91 / 100</strong>, with every finding public, including the ones we are not
              proud of. If you want production-hardened today, wait; if you want to watch a
              framework grow with its test suite open, welcome.
            </p>
            <p style={{ marginTop: 10 }}>
              <a href="/blog/auditing-our-own-framework-91-of-100">Read the audit post →</a>
            </p>
          </div>
        </div>
      </section>

      {latest.length > 0 && (
        <section className="section">
          <div className="container">
            <h2>From the blog</h2>
            <p className="lede">
              <a href="/blog">All posts →</a>
            </p>
            <div className="posts">
              {latest.map((p) => (
                <a className="post-card" href={`/blog/${p.slug}`} key={p.slug}>
                  <div className="meta">
                    <span>{p.date}</span>
                  </div>
                  <h3>{p.title}</h3>
                  <p>{p.description}</p>
                </a>
              ))}
            </div>
          </div>
        </section>
      )}
    </main>
  )
}
