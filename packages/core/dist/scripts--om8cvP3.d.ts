/** Maximum payload size (matches Rust `MAX_BYTES`). */
declare const MAX_BYTES: number;
/** Maximum nesting depth (matches Rust `MAX_DEPTH`). */
declare const MAX_DEPTH = 512;
/** Maximum shared-slot table entries (matches Rust `MAX_REFS`). */
declare const MAX_REFS: number;
/** Error thrown by the decoder; `code` is a stable ODF-NNN diagnostic code. */
declare class OdfError extends Error {
  code: string;
  constructor(code: string, message: string);
}
/**
 * Decode ODF bytes into rich JS values (see the module header for the mapping).
 * Throws [OdfError] on any malformed, truncated or over-limit payload.
 */
declare function decodeOdf(bytes: Uint8Array): unknown;
/**
 * Decode and project to a JSON-serializable shape (mirrors Rust `Value::to_json`):
 * undefined→null, NaN/±Inf→null, Date→epoch ms number, BigInt→decimal string,
 * URL→href, RegExp→source, Map→object with Rust-Debug-formatted keys,
 * Set→array, typed array→byte array, Error→{name, message}, resolved Refs→
 * null on revisit (cycles cut, like Value::Ref.to_json).
 */
declare function decodeOdfToJson(bytes: Uint8Array): unknown;
/** A single reported sample. */
interface VitalSample {
  /** `LCP` | `CLS` | `INP` | `FCP` | `TTFB` (or a custom name). */
  name: string;
  /** Milliseconds for the timing metrics; unitless for CLS. */
  value: number;
  /** The path the sample was measured on (`location.pathname` by default). */
  route: string;
}
interface VitalsOptions {
  /** Endpoint to POST to. Default `/_oxide/vitals`. */
  endpoint?: string;
  /** Route label; default `location.pathname`. */
  route?: string;
  /** Which metrics to collect. Default: all five. */
  metrics?: string[];
  /**
   * Called with each sample before it is queued. A hook for a project that
   * wants to forward vitals to its own analytics alongside the Oxide endpoint.
   * Throwing here is ignored; reporting still proceeds.
   */
  onSample?: (s: VitalSample) => void;
  /** Disable the network report (e.g. a test or a privacy mode). */
  report?: boolean;
}
/**
 * Install the reporting hook. Returns a disposer that disconnects the
 * observers and clears any pending timer. Idempotent per call — calling twice
 * installs two independent hooks (the caller owns the disposer).
 *
 * No-op (returns a no-op disposer) outside the browser.
 */
declare function reportWebVitals(opts?: VitalsOptions): () => void;
/** The strategies §16.6 defines, plus two common conveniences. */
type ScriptStrategy = 'defer' | 'idle' | 'worker' | 'afterInteractive' | 'lazy';
interface ScriptOptions {
  /** The document to operate on. Defaults to `globalThis.document`. */
  doc?: Document;
  /** A script's source URL (a `src` script). Omit for inline code. */
  src?: string;
  /** A script's inline body. Omit for a `src` script. */
  content?: string;
  /** Module scripts get `type="module"`. Default `false` (classic). */
  module?: boolean;
  /** Arbitrary attributes copied onto the injected `<script>` element. */
  attrs?: Record<string, string>;
  /** Called after the script loads (a `src` script's `load` event). */
  onLoad?: () => void;
  /** Called if the script fails to load. */
  onError?: (e: unknown) => void;
  /** Overridable scheduling primitives (for tests). */
  requestIdleCallback?: (cb: () => void) => unknown;
  IntersectionObserver?: new (cb: (entries: Array<{
    isIntersecting: boolean;
  }>) => void) => {
    observe(el: Element): void;
    disconnect(): void;
  };
  Worker?: new (url: string) => {
    postMessage(msg: unknown): void;
    terminate(): void;
  };
}
/**
 * Schedule one script by strategy. Returns a disposer (a no-op for most
 * strategies; it disconnects the IntersectionObserver for `lazy`). No-op when
 * no document is present.
 */
declare function scheduleScript(strategy: ScriptStrategy, opts?: ScriptOptions): () => void;
/**
 * Scan the document for server-rendered `<script data-ox-script>` tags and
 * schedule each by its `data-ox-strategy`. The generated boot entry calls
 * this after `boot()`.
 *
 * A tag is a *placeholder*: it carries `data-ox-src` (a URL) or
 * `data-ox-inline` (base64 body) plus the strategy. The real script is
 * injected by `scheduleScript`; the placeholder stays inert.
 */
declare function loadScripts(opts?: ScriptOptions): void;
export { VitalSample as a, MAX_BYTES as c, OdfError as d, decodeOdf as f, scheduleScript as i, MAX_DEPTH as l, ScriptStrategy as n, VitalsOptions as o, decodeOdfToJson as p, loadScripts as r, reportWebVitals as s, ScriptOptions as t, MAX_REFS as u };
//# sourceMappingURL=scripts--om8cvP3.d.ts.map