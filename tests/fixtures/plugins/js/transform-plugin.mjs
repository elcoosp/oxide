// A minimal `kind:js` plugin proving the transform hook is wired end to end.
// It rewrites the page's rendered text, so the built chunk carries the new
// string — that is the gate's assertion. A comment would be stripped by
// minification; a changed string literal is not.
export function transform(code) {
  return code.replace(/plugin-fixture/g, 'TRANSFORM_PLUGIN_MARKER');
}
