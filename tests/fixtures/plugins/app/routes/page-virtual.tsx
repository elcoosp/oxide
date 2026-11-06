// Page fixture for the virtual-module plugin case (§15-F): imports the
// specifier that virtual-plugin.mjs fabricates via resolveId/load and renders
// its value. The build must prove the two bundler hooks reached the pipeline
// — if resolveId/load never fire, the import fails to resolve and the build
// errors; if their result is not wired through, the marker text is missing
// from the SSR markup.
import { value } from '#virtual'

export default function Page() {
  return <h1 id="virtual-marker">{value}</h1>
}
