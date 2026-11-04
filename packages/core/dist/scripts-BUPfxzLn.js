// src/client/scripts.ts
function docOf(opts) {
	return opts.doc ?? globalThis.document;
}
function idle(cb, opts) {
	const ric = opts.requestIdleCallback ?? globalThis.requestIdleCallback;
	if (typeof ric === "function") ric(cb);
	else setTimeout(cb, 1);
}
/** Inject a `<script>` element and resolve when it loads (or run inline). */
function inject(doc, opts) {
	const el = doc.createElement("script");
	if (opts.module) el.type = "module";
	if (opts.attrs) for (const [k, v] of Object.entries(opts.attrs)) el.setAttribute(k, v);
	if (opts.src) {
		el.src = opts.src;
		if (opts.onLoad) el.addEventListener("load", opts.onLoad, { once: true });
		if (opts.onError) el.addEventListener("error", (e) => opts.onError?.(e), { once: true });
		doc.head.appendChild(el);
	} else {
		el.textContent = opts.content ?? "";
		doc.head.appendChild(el);
		opts.onLoad?.();
	}
}
/** Run a script body in a Web Worker; a `src` is `importScripts`ed there. */
function runInWorker(opts) {
	const W = opts.Worker ?? globalThis.Worker;
	if (typeof W !== "function") {
		const doc = docOf(opts);
		if (doc) inject(doc, opts);
		return;
	}
	const body = opts.src ? `importScripts(${JSON.stringify(opts.src)});` : opts.content ?? "";
	try {
		const blob = new Blob([body], { type: "application/javascript" });
		const url = URL.createObjectURL(blob);
		const worker = new W(url);
		setTimeout(() => {
			worker.terminate();
			URL.revokeObjectURL(url);
		}, 5e3);
		opts.onLoad?.();
	} catch (e) {
		opts.onError?.(e);
	}
}
/**
* Schedule one script by strategy. Returns a disposer (a no-op for most
* strategies; it disconnects the IntersectionObserver for `lazy`). No-op when
* no document is present.
*/
function scheduleScript(strategy, opts = {}) {
	const doc = docOf(opts);
	if (!doc) return () => {};
	switch (strategy) {
		case "defer":
			if (doc.readyState === "loading") doc.addEventListener("DOMContentLoaded", () => inject(doc, opts), { once: true });
			else inject(doc, opts);
			return () => {};
		case "idle":
			idle(() => inject(doc, opts), opts);
			return () => {};
		case "afterInteractive":
			Promise.resolve().then(() => inject(doc, opts));
			return () => {};
		case "worker":
			idle(() => runInWorker(opts), opts);
			return () => {};
		case "lazy": {
			const IO = opts.IntersectionObserver ?? globalThis.IntersectionObserver;
			const placeholder = doc.currentScript ?? doc.querySelector("[data-ox-script-placeholder]");
			if (typeof IO !== "function" || !placeholder) {
				idle(() => inject(doc, opts), opts);
				return () => {};
			}
			const obs = new IO((entries) => {
				if (entries.some((e) => e.isIntersecting)) {
					obs.disconnect();
					inject(doc, opts);
				}
			});
			obs.observe(placeholder);
			return () => obs.disconnect();
		}
	}
}
/**
* Scan the document for server-rendered `<script data-ox-script>` tags and
* schedule each by its `data-ox-strategy`. The generated boot entry calls
* this after `boot()`.
*
* A tag is a *placeholder*: it carries `data-ox-src` (a URL) or
* `data-ox-inline` (base64 body) plus the strategy. The real script is
* injected by `scheduleScript`; the placeholder stays inert.
*/
function loadScripts(opts = {}) {
	const doc = docOf(opts);
	if (!doc) return;
	const tags = doc.querySelectorAll("script[data-ox-script]");
	for (let i = 0; i < tags.length; i++) {
		const el = tags[i];
		if (el === void 0) continue;
		const strategy = el.getAttribute("data-ox-strategy") ?? "defer";
		const src = el.getAttribute("data-ox-src") ?? void 0;
		const inlineB64 = el.getAttribute("data-ox-inline");
		const content = inlineB64 ? new TextDecoder().decode(Uint8Array.from(atob(inlineB64), (c) => c.charCodeAt(0))) : void 0;
		const isModule = el.getAttribute("data-ox-module") !== null;
		scheduleScript(strategy, {
			...opts,
			doc,
			src,
			content,
			module: isModule
		});
	}
}
export { scheduleScript as n, loadScripts as t };

//# sourceMappingURL=scripts-BUPfxzLn.js.map