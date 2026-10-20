// A plugin that virtualizes `#virtual`. `resolveId` maps the specifier to a
// synthetic id; `load` returns its source. Proves both hooks reach the
// bundler — the page imports `#virtual` and renders its value.
const VIRTUAL_ID = '\0virtual-module';

export function resolveId(specifier) {
  return specifier === '#virtual' ? VIRTUAL_ID : null;
}

export function load(id) {
  return id === VIRTUAL_ID ? 'export const value = "from-virtual-plugin";\n' : null;
}
