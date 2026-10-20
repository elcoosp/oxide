// Reports the emitted files unchanged, but the fact that it runs is the
// proof: an unset hook means the bundler's post-emit pass never calls in.
export function generateBundle(files) {
  return files;
}
