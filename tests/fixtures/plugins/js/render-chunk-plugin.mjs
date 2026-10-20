// Appends a marker to every emitted chunk. Proves `renderChunk` runs after
// Rolldown writes the file.
export function renderChunk(file, code) {
  return `${code}\n// render-chunk-fixture\n`;
}
