// src/routes/layout.tsx — root layout (§8.2). The adapter shell owns the
// <html>/<body> document; layouts compose the app chrome around pages.
// Importing the stylesheet here is enough: the bundler hashes it through
// Lightning CSS and the injector streams the <link> into every page's head.
import type { ReactNode } from 'react'
import '../styles/tailwind.css'
import { Island } from '@oxide-fw/react'
import { site, nav, footerLinks } from '../lib/site.ts'

import { Github } from 'lucide-react';
interface Props {
  children?: ReactNode
}

export default function RootLayout({ children }: Props) {
  return (
    <>
      <link rel="preconnect" href="https://fonts.googleapis.com" />
      <link rel="preconnect" href="https://fonts.gstatic.com" crossOrigin="anonymous" />
      <link href="https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@400;500;700&display=swap" rel="stylesheet" />
      <link rel="icon" type="image/png" href="/logo.png" />


      

      <Island component="RevealBootstrap" chunk="RevealBootstrap" strategy="visible" />
      <Island component="FrameworkTabsBootstrap" chunk="FrameworkTabsBootstrap" strategy="visible" />
      
      
      
{/* ═══ FrameworkAdapters CSS (inlined by wr14) ═══ */}
      

      <div className="scroll-progress" aria-hidden="true" />
      <header className="sticky top-0 z-50 w-full border-b border-zinc-800 bg-[#09090b]/80 backdrop-blur-md">
        <div className="max-w-6xl mx-auto px-6 h-16 flex items-center justify-between">
          <a href="/" className="flex items-center gap-2.5 group">
            <span className="flex items-center justify-center w-10 h-10 rounded-full bg-zinc-100 shadow-[inset_0_1px_0_rgba(255,255,255,0.6),0_0_0_1px_rgba(250,250,250,0.08),0_2px_8px_rgba(0,0,0,0.4)] group-hover:shadow-[inset_0_1px_0_rgba(255,255,255,0.8),0_0_0_1px_rgba(228,86,49,0.4),0_2px_12px_rgba(228,86,49,0.3)] transition-all duration-300">
              <img src="/logo.png" alt="Oxide" className="w-8 h-8 object-contain" />
            </span>
            <span className="font-bold tracking-tight text-xl text-zinc-50">
              oxide<span className="text-[#e45631]">_</span>
            </span>
          </a>

          <nav className="flex items-center gap-8">
            <div className="hidden md:flex items-center gap-6">
              <a href="/docs" className="text-sm font-medium text-zinc-400 hover:text-zinc-50 transition-colors">Docs</a>
              <a href="/blog" className="text-sm font-medium text-zinc-400 hover:text-zinc-50 transition-colors">Blog</a>
            </div>
            <a href={site.repo} target="_blank" rel="noopener" className="flex items-center gap-2 text-sm font-medium text-zinc-300 bg-zinc-800/50 hover:bg-zinc-800 border border-zinc-700/50 hover:border-zinc-600 px-4 py-1.5 rounded-lg transition-all">
              <Github size={16} />
              <span>GitHub</span>
            </a>
          </nav>
        </div>
      </header>

      {children}

      <footer className="border-t border-zinc-800 bg-[#09090b] pt-16 pb-8">
        <div className="max-w-6xl mx-auto px-6">
          <div className="grid grid-cols-1 md:grid-cols-4 gap-12 mb-16">

            <div className="col-span-1 md:col-span-1">
              <a href="/" className="flex items-center gap-2.5 group">
            <span className="flex items-center justify-center w-10 h-10 rounded-full bg-zinc-100 shadow-[inset_0_1px_0_rgba(255,255,255,0.6),0_0_0_1px_rgba(250,250,250,0.08),0_2px_8px_rgba(0,0,0,0.4)] group-hover:shadow-[inset_0_1px_0_rgba(255,255,255,0.8),0_0_0_1px_rgba(228,86,49,0.4),0_2px_12px_rgba(228,86,49,0.3)] transition-all duration-300">
              <img src="/logo.png" alt="Oxide" className="w-8 h-8 object-contain" />
            </span>
            <span className="font-bold tracking-tight text-xl text-zinc-50">
              oxide<span className="text-[#e45631]">_</span>
            </span>
          </a>
              <p className="text-sm text-zinc-500 leading-relaxed max-w-xs mt-4">
                A full-stack meta-framework with a Rust core. One engine, every framework.
              </p>
            </div>

            <div>
              <h4 className="font-mono text-xs tracking-widest uppercase text-zinc-300 mb-4 font-semibold">Product</h4>
              <ul className="space-y-3 text-sm text-zinc-500">
                <li><a href="/docs" className="hover:text-[#e45631] transition-colors">Documentation</a></li>
                <li><a href="/docs/getting-started" className="hover:text-[#e45631] transition-colors">Getting Started</a></li>
                <li><a href="/blog" className="hover:text-[#e45631] transition-colors">Blog</a></li>
              </ul>
            </div>

            <div>
              <h4 className="font-mono text-xs tracking-widest uppercase text-zinc-300 mb-4 font-semibold">Community</h4>
              <ul className="space-y-3 text-sm text-zinc-500">
                <li><a href={site.repo} className="flex items-center gap-2 hover:text-[#e45631] transition-colors"><Github size={16} /> GitHub</a></li>
                <li><a href="/rss" className="hover:text-[#e45631] transition-colors">RSS Feed</a></li>
                <li><a href={`${site.repo}/blob/main/LICENSE-MIT`} className="hover:text-[#e45631] transition-colors">MIT License</a></li>
                <li><a href={`${site.repo}/blob/main/LICENSE-APACHE`} className="hover:text-[#e45631] transition-colors">Apache-2.0 License</a></li>
              </ul>
            </div>

            <div>
              <h4 className="font-mono text-xs tracking-widest uppercase text-zinc-300 mb-4 font-semibold">Ready to ship?</h4>
              <p className="text-sm text-zinc-500 mb-4">Start building your next app today.</p>
              <a className="btn btn-primary w-full text-center" href="/docs">Get started</a>
            </div>

          </div>

          <div className="border-t border-zinc-800/50 pt-8 flex flex-col md:flex-row items-center justify-between gap-4 text-xs font-mono text-zinc-600">
            <div className="flex items-center gap-4">
              <span>Oxide v0.1</span>
              <span className="w-1 h-1 rounded-full bg-zinc-700"></span>
              <span>MIT / Apache-2.0</span>
            </div>
            <div>
              This site is rendered by <a href="/" className="text-zinc-400 hover:text-[#e45631] transition-colors">oxide itself</a> — one binary, streaming SSR.
            </div>
          </div>
        </div>
      </footer>
    </>
  )
}
