// §7.1 / §9.2: resolve `event.defer()` placeholders in loader data.
//
// A loader returns immediately with `{ "__deferred": id }` placeholders; the
// pipeline collects the settle stream and hands the resolved map to the
// adapter as `RenderInput.deferred`. Every render path that builds a
// component tree — the real adapter and each framework's IR renderer — must
// substitute before the components run, or they receive the placeholder
// object instead of the value.

export function resolveDeferredPlaceholders(
  value: unknown,
  deferred: Record<string, unknown> | undefined,
  depth = 1,
  seen = new WeakSet<object>(),
): unknown {
  if (depth > 512 || value === null || typeof value !== 'object') return value
  if (seen.has(value)) return value
  seen.add(value)

  if (Array.isArray(value)) {
    for (let i = 0; i < value.length; i++) {
      value[i] = resolveDeferredPlaceholders(value[i], deferred, depth + 1, seen)
    }
    return value
  }
  const maybePlaceholder = value as { __deferred?: unknown }
  if (
    typeof maybePlaceholder.__deferred === 'number' &&
    Object.keys(value as object).length === 1
  ) {
    const id = String(maybePlaceholder.__deferred)
    if (deferred && Object.prototype.hasOwnProperty.call(deferred, id)) {
      return deferred[id]
    }
    return value
  }
  if (value instanceof Map) {
    for (const [k, v] of value) {
      value.set(k, resolveDeferredPlaceholders(v, deferred, depth + 1, seen))
    }
    return value
  }
  if (value instanceof Set) {
    return new Set([...value].map((v) => resolveDeferredPlaceholders(v, deferred, depth + 1, seen)))
  }
  const obj = value as Record<string, unknown>
  for (const k of Object.keys(obj)) {
    if (k === '__proto__' || k === 'constructor') continue
    obj[k] = resolveDeferredPlaceholders(obj[k], deferred, depth + 1, seen)
  }
  return obj
}
