// §V2-E06: the analytics fixture app's oxide.config.ts — the file the
// `oxide add` config edit operates on (idempotent, comment-preserving,
// refuses a dynamic config). The example (`crates/cli/examples/module_add.rs`)
// loads this fixture and asserts the AST edit lands the import + the
// modules-array element.

import { defineConfig } from '@oxide-fw/core'
// The comment below must survive the AST edit (it pins comment preservation).
//   placeholder: do not remove
export default defineConfig({
  framework: { adapter: 'react' },
  routes: { dir: 'src/routes' },
})
