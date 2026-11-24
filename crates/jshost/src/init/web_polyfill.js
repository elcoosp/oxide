// V2-A08: minimal WHATWG web globals for the V8 isolate — `Headers`,
// `Request`, `Response`.
//
// deno_core does not register the web globals; the bootstrap polyfills the
// render-tier surface (TextEncoder/ReadableStream/URL), but the §8.4
// endpoint and §8.1 middleware handlers speak the fetch-object dialect: the
// Node worker builds a real `Request` and reads a real `Response`
// (`instanceof Response`, `.headers.forEach`, `.body.getReader`,
// `.arrayBuffer()`). Before this polyfill those handlers could not even
// evaluate on the V8 backend ("Response is not defined").
//
// The surface is exactly the one the worker's `dispatchEndpointRun` /
// `dispatchMiddlewareRun` and the op-parity fixtures exercise:
// - `Headers`: pair-iterable / object / `Headers` init; get/has/set/append/
//   delete/forEach. `forEach(value, key)` is the projection the worker uses.
// - `Request`: `(url, {method, headers, body})` — `.url`, `.method`,
//   `.headers`, `.json()`, `.text()`, `.arrayBuffer()`.
// - `Response`: `(body, {status, headers})` — `.status`, `.headers`,
//   `.body` (a `ReadableStream` for any non-null body, mirroring Node, so
//   the worker's streaming arm is taken on both backends for the same
//   handlers), `.arrayBuffer()`, `.text()`. A `null` body keeps
//   `response.body === null` (the buffered arm).
//
// IMPORTANT: keep this file pure 7-bit ASCII (deno_core's
// ExtensionFileSource::new_computed() panics on non-ASCII source — see the
// url_polyfill note). Use \uXXXX escapes inside JS strings if a Unicode
// character is ever needed.
(function () {
  if (typeof globalThis.Headers === 'function' &&
      typeof globalThis.Request === 'function' &&
      typeof globalThis.Response === 'function') return;

  class OxideHeaders {
    constructor(init) {
      this._pairs = [];
      if (!init) return;
      if (init instanceof OxideHeaders) {
        this._pairs = init._pairs.map(function (p) { return [p[0], p[1]]; });
        return;
      }
      if (Array.isArray(init) || (init && typeof init[Symbol.iterator] === 'function')) {
        for (const pair of init) {
          if (!Array.isArray(pair) || pair.length < 2) continue;
          this.append(String(pair[0]), String(pair[1]));
        }
        return;
      }
      if (typeof init === 'object') {
        for (const k of Object.keys(init)) this.append(k, String(init[k]));
      }
    }
    append(key, value) {
      this._pairs.push([String(key), String(value)]);
    }
    set(key, value) {
      const k = String(key);
      const v = String(value);
      const out = [];
      let found = false;
      for (const p of this._pairs) {
        if (p[0].toLowerCase() === k.toLowerCase()) {
          if (!found) { out.push([k, v]); found = true; }
          continue;
        }
        out.push(p);
      }
      if (!found) out.push([k, v]);
      this._pairs = out;
    }
    get(key) {
      const k = String(key).toLowerCase();
      const hit = this._pairs.find(function (p) { return p[0].toLowerCase() === k; });
      return hit ? hit[1] : null;
    }
    has(key) {
      return this.get(key) !== null;
    }
    delete(key) {
      const k = String(key).toLowerCase();
      this._pairs = this._pairs.filter(function (p) { return p[0].toLowerCase() !== k; });
    }
    forEach(callback, thisArg) {
      const self = thisArg ?? this;
      for (const p of this._pairs) callback.call(self, p[1], p[0], this);
    }
    entries() {
      const pairs = this._pairs;
      let index = 0;
      return {
        next: function () {
          if (index >= pairs.length) return { value: undefined, done: true };
          const p = pairs[index++];
          return { value: [p[0], p[1]], done: false };
        },
        [Symbol.iterator]: function () { return this; },
      };
    }
  }

  function bytesToStream(bytes) {
    // The bootstrap's own ReadableStream polyfill (start-produced queue);
    // the endpoint bootstrap drains it through `getReader()`.
    return new globalThis.ReadableStream({
      start: function (controller) {
        if (bytes.length > 0) controller.enqueue(bytes);
        controller.close();
      },
    });
  }

  function bodyBytesOf(body) {
    if (body == null) return null;
    if (body instanceof Uint8Array) return new Uint8Array(body);
    if (typeof body === 'string') return new TextEncoder().encode(body);
    return null;
  }

  class OxideRequest {
    constructor(url, init) {
      init = init ?? {};
      this.url = String(url);
      const method = String(init.method ?? 'GET').toUpperCase();
      this.method = method.length > 0 ? method : 'GET';
      this.headers = new OxideHeaders(init.headers ?? []);
      const bytes = bodyBytesOf(init.body);
      // A GET/HEAD request carries no body even when one was passed.
      this._bytes = this.method === 'GET' || this.method === 'HEAD' ? null : bytes;
    }
    arrayBuffer() {
      const bytes = this._bytes ?? new Uint8Array(0);
      return Promise.resolve(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength));
    }
    text() {
      const bytes = this._bytes ?? new Uint8Array(0);
      return Promise.resolve(new TextDecoder().decode(bytes));
    }
    json() {
      return this.text().then(function (t) { return JSON.parse(t); });
    }
  }

  class OxideResponse {
    constructor(body, init) {
      init = init ?? {};
      const status = init.status;
      this.status = typeof status === 'number' ? status : 200;
      this.headers = new OxideHeaders(init.headers ?? []);
      if (body != null && typeof body.getReader === 'function') {
        // A stream body passes through; there are no buffered bytes.
        this.body = body;
        this._bytes = null;
        return;
      }
      const bytes = bodyBytesOf(body);
      if (bytes == null) {
        // Node keeps `body === null` for a body-less Response; the endpoint
        // bootstrap takes its buffered arm for that shape.
        this.body = null;
        this._bytes = new Uint8Array(0);
        return;
      }
      this._bytes = bytes;
      // Node's Response exposes a ReadableStream body for any non-null
      // body (a string included) — mirror that so both backends take the
      // worker's streaming arm for the same handler code.
      this.body = bytesToStream(bytes);
    }
    arrayBuffer() {
      const self = this;
      if (self._bytes != null) {
        const bytes = self._bytes;
        return Promise.resolve(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength));
      }
      if (self.body != null && typeof self.body.getReader === 'function') {
        return Promise.resolve().then(function () {
          const reader = self.body.getReader();
          const chunks = [];
          function pump() {
            return reader.read().then(function (step) {
              if (step.done) {
                let total = 0;
                for (const c of chunks) total += c.length;
                const out = new Uint8Array(total);
                let at = 0;
                for (const c of chunks) { out.set(c, at); at += c.length; }
                return out.buffer;
              }
              chunks.push(step.value);
              return pump();
            });
          }
          return pump();
        });
      }
      return Promise.resolve(new ArrayBuffer(0));
    }
    text() {
      return this.arrayBuffer().then(function (buf) {
        return new TextDecoder().decode(new Uint8Array(buf));
      });
    }
  }

  if (typeof globalThis.Headers !== 'function') globalThis.Headers = OxideHeaders;
  if (typeof globalThis.Request !== 'function') globalThis.Request = OxideRequest;
  if (typeof globalThis.Response !== 'function') globalThis.Response = OxideResponse;
})();
