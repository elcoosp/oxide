// src/lib/site.ts — site-wide constants shared by pages, layouts and the
// RSS endpoint. Every number here is a verified fact from the project's own
// test suites and public spec-compliance audit (round 7).

export const site = {
  name: 'Oxide',
  tagline: 'One Rust core. Every framework.',
  description:
    'A framework-agnostic full-stack meta-framework with a Rust core: real streaming SSR for React, Vue, Svelte and Solid, served from a single static binary with HTTP/3 and a byte-parity cache.',
  url: 'https://oxide.dev',
  repo: 'https://github.com/elcoosp/oxide',
  repoSpec: 'https://github.com/elcoosp/oxide/blob/main/docs/Oxide%20Technical%20Specification.md',
  version: 'v0.1',
  license: 'MIT | Apache-2.0',
} as const

/** Verified proof points — keep in lockstep with the repo README badges. */
export const stats = [
  { value: 1251, label: 'Rust test functions' },
  { value: 4, label: 'framework adapters' },
  { value: 22, label: 'CI jobs, all platforms' },
  { value: 12, label: 'deploy targets' },
] as const

export const nav = [
  { href: '/docs', label: 'Docs' },
  { href: '/blog', label: 'Blog' },
] as const

export const footerLinks = [
  { href: site.repo, label: 'GitHub' },
  { href: '/rss', label: 'RSS' },
] as const
