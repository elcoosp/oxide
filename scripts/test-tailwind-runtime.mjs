// Standalone test for @oxide-fw/tailwind/runtime.mjs — no Oxide build required.
// Run: node scripts/test-tailwind-runtime.mjs

import { ping, scan, build } from '../packages/tailwind/runtime.mjs'

console.log('1. ping()')
const pingResult = await ping()
console.log('   ', pingResult)
if (!pingResult.ok) throw new Error('ping failed')

console.log('2. scan()')
const candidates = await scan({
  sources: [],
  files: [
    { content: '<div className="flex bg-red-500 px-4">x</div>', extension: 'tsx' },
    { content: '<span class="hover:bg-blue-500">y</span>', extension: 'html' },
  ],
})
console.log('   candidates:', candidates)
for (const expected of ['flex', 'bg-red-500', 'px-4']) {
  if (!candidates.includes(expected)) {
    throw new Error(`scan: expected '${expected}' in ${JSON.stringify(candidates)}`)
  }
}

console.log('3. build()')
const result = await build({ css: '@import "tailwindcss";', base: process.cwd(), candidates })
console.log('   css length:', result.css.length)
for (const expected of ['.flex', '.bg-red-500', '.px-4']) {
  if (!result.css.includes(expected)) throw new Error(`build: missing '${expected}'`)
}

console.log('4. build() with @theme customization')
const themed = await build({
  css: `@import "tailwindcss";\n@theme { --color-brand: #e8734a; }`,
  base: process.cwd(),
  candidates: ['text-brand'],
})
console.log('   css length:', themed.css.length)
if (!themed.css.includes('--color-brand')) throw new Error('build: @theme not applied')

console.log('')
console.log('ALL PASS')
