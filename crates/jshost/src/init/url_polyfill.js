// Minimal WHATWG-URL polyfill for the V8 isolate.
// deno_core does not ship URL by default; middleware and user code that
// reads `new URL(request.url)` need it. Covers the surface middleware
// actually uses: constructor, .searchParams (get/set/has/append/delete),
// .pathname, .search, .hash, .origin, .href. Not byte-complete WHATWG
// URL -- enough to unblock the common patterns without pulling deno_url.
//
// IMPORTANT: this file must be pure 7-bit ASCII. deno_core's
// ExtensionFileSource::new_computed() panics on non-ASCII source, and
// that panic surfaces as "JsRuntime::new panicked" -- which is almost
// impossible to trace back to its actual cause at runtime. If a future
// edit needs a Unicode character, write it as a \uXXXX escape inside a
// JS string; do not paste the actual character.
(function () {
  if (typeof globalThis.URL === 'function' &&
      typeof globalThis.URLSearchParams === 'function') return;

  function URLSearchParams(init) {
    this._pairs = [];
    if (typeof init === 'string') {
      const s = init.replace(/^\?/, '');
      if (s) {
        for (const pair of s.split('&')) {
          if (!pair) continue;
          const i = pair.indexOf('=');
          const k = i < 0 ? pair : pair.slice(0, i);
          const v = i < 0 ? '' : pair.slice(i + 1);
          this._pairs.push([
            decodeURIComponent(k.replace(/\+/g, ' ')),
            decodeURIComponent(v.replace(/\+/g, ' ')),
          ]);
        }
      }
    } else if (init && typeof init === 'object') {
      for (const k of Object.keys(init)) {
        const v = init[k];
        if (Array.isArray(v)) for (const x of v) this._pairs.push([String(k), String(x)]);
        else this._pairs.push([String(k), String(v)]);
      }
    } else if (init && typeof init[Symbol.iterator] === 'function') {
      for (const pair of init) this._pairs.push([String(pair[0]), String(pair[1])]);
    }
  }
  URLSearchParams.prototype.get = function (name) {
    for (const [k, v] of this._pairs) if (k === name) return v;
    return null;
  };
  URLSearchParams.prototype.getAll = function (name) {
    const out = [];
    for (const [k, v] of this._pairs) if (k === name) out.push(v);
    return out;
  };
  URLSearchParams.prototype.has = function (name) {
    for (const [k] of this._pairs) if (k === name) return true;
    return false;
  };
  URLSearchParams.prototype.set = function (name, value) {
    name = String(name); value = String(value);
    let replaced = false;
    this._pairs = this._pairs.filter(([k]) => {
      if (k !== name) return true;
      if (!replaced) { replaced = true; return true; }
      return false;
    });
    if (!replaced) this._pairs.push([name, value]);
    else {
      for (const p of this._pairs) if (p[0] === name) { p[1] = value; break; }
    }
  };
  URLSearchParams.prototype.append = function (name, value) {
    this._pairs.push([String(name), String(value)]);
  };
  URLSearchParams.prototype.delete = function (name) {
    this._pairs = this._pairs.filter(([k]) => k !== name);
  };
  URLSearchParams.prototype.toString = function () {
    return this._pairs
      .map(([k, v]) => encodeURIComponent(k) + '=' + encodeURIComponent(v))
      .join('&');
  };
  URLSearchParams.prototype.forEach = function (cb) {
    for (const [k, v] of this._pairs) cb(v, k, this);
  };
  Object.defineProperty(URLSearchParams.prototype, Symbol.iterator, {
    value: function* () { yield* this._pairs; },
  });

  const URL_RE =
    /^([a-zA-Z][a-zA-Z0-9+.\-]*):(?:(\/\/)([^/?#]*))?([^?#]*)(?:\?([^#]*))?(?:#(.*))?$/;

  function URL(input, base) {
    if (!(this instanceof URL)) throw new TypeError('URL constructor called without new');
    input = String(input);
    if (base !== undefined) {
      const b = base instanceof URL ? base : new URL(String(base));
      if (!/^[a-zA-Z][a-zA-Z0-9+.\-]*:/.test(input)) {
        if (input.startsWith('//')) input = b.protocol + input;
        else if (input.startsWith('/')) input = b.protocol + '//' + b.host + input;
        else {
          const dir = b.pathname.replace(/[^/]*$/, '');
          input = b.protocol + '//' + b.host + dir + input;
        }
      }
    }
    const m = URL_RE.exec(input);
    if (!m) throw new TypeError('Invalid URL: ' + input);
    const protocol = m[1].toLowerCase() + ':';
    const authority = m[3] || '';
    const at = authority.lastIndexOf('@');
    const userinfo = at >= 0 ? authority.slice(0, at) : '';
    const hostport = at >= 0 ? authority.slice(at + 1) : authority;
    const colon = hostport.lastIndexOf(':');
    const hostname = colon >= 0 ? hostport.slice(0, colon) : hostport;
    const port = colon >= 0 ? hostport.slice(colon + 1) : '';
    this.protocol = protocol;
    this.username = userinfo.split(':')[0] || '';
    this.password = userinfo.split(':')[1] || '';
    this.hostname = hostname;
    this.port = port;
    this.host = hostport;
    this.pathname = m[4] || '/';
    this.search = m[5] !== undefined ? '?' + m[5] : '';
    this.hash = m[6] !== undefined ? '#' + m[6] : '';
    this._sp = null;
    this.origin = (protocol === 'http:' || protocol === 'https:' || protocol === 'ws:' || protocol === 'wss:')
      ? protocol + '//' + this.host : 'null';
  }
  Object.defineProperty(URL.prototype, 'href', {
    get() { return this.protocol + '//' + this.host + this.pathname + this.search + this.hash; },
    set(v) {
      const u = new URL(String(v));
      for (const k of ['protocol','username','password','hostname','port','host','pathname','search','hash','origin','_sp']) {
        this[k] = u[k];
      }
    },
  });
  Object.defineProperty(URL.prototype, 'searchParams', {
    get() {
      if (!this._sp) this._sp = new URLSearchParams(this.search);
      return this._sp;
    },
  });
  URL.prototype.toString = function () { return this.href; };
  URL.prototype.toJSON = function () { return this.href; };

  globalThis.URL = URL;
  globalThis.URLSearchParams = URLSearchParams;
})();
