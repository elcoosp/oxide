// The CLI's generateBundle hook is a post-emit pass over the emitted chunk
// list (see `crates/bundler/src/rolldown_backend.rs`): it receives the file
// names and may veto/reorder them. This fixture returns the list unchanged
// but writes a marker file from inside the plugin — an e2e build that ran
// the hook leaves the marker behind, which the fixture gate asserts. (A veto
// itself is only observable in the manifests, and vetoing a real chunk would
// break the build's own later phases.)
import { writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

export function generateBundle(files) {
  const here = dirname(fileURLToPath(import.meta.url));
  writeFileSync(join(here, '.generate-bundle-ran'), `${files.length} files\n`);
  return files;
}
