// Injects a marker into every prerendered HTML document. Proves
// `transformHtml` runs on the finalizer output.
export function transformHtml(route, html) {
  return html.replace('</body>', '<!-- transform-html-fixture --></body>');
}
